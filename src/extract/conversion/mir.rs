use std::collections::VecDeque;

use mlua_typegen::{LuaLiteral, LuaTableField, LuaTableShape, LuaValueDomain, TableOpenness};
use rustc_hir::def_id::DefId;
use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::{
    AggregateKind, BasicBlock, Body, Local, Location, Operand, Place, PlaceElem, Rvalue,
    StatementKind, TerminatorKind,
};
use rustc_middle::ty::{self, Ty, TyCtxt};

use super::{ConversionDirection, UnsupportedConversion};

pub(super) fn analyze_domain<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    rust_type: &str,
    direction: ConversionDirection,
) -> Result<LuaValueDomain, UnsupportedConversion> {
    MirAnalyzer::new(tcx, body, rust_type).run(direction)
}

#[derive(Clone, Debug)]
enum Definition<'tcx> {
    Use(Operand<'tcx>),
    Ref(Place<'tcx>),
    Call {
        callee: DefId,
        args: Vec<Operand<'tcx>>,
    },
}

struct MirAnalyzer<'a, 'tcx> {
    tcx: TyCtxt<'tcx>,
    body: &'a Body<'tcx>,
    reachable: Vec<bool>,
    definitions: Vec<Vec<Definition<'tcx>>>,
    rust_type: &'a str,
    depth: usize,
}

impl<'a, 'tcx> MirAnalyzer<'a, 'tcx> {
    fn new(tcx: TyCtxt<'tcx>, body: &'a Body<'tcx>, rust_type: &'a str) -> Self {
        Self::with_depth(tcx, body, rust_type, 0)
    }

    fn with_depth(
        tcx: TyCtxt<'tcx>,
        body: &'a Body<'tcx>,
        rust_type: &'a str,
        depth: usize,
    ) -> Self {
        let reachable = reachable_blocks(body);
        let mut definitions = vec![Vec::new(); body.local_decls.len()];

        for (block, data) in body.basic_blocks.iter_enumerated() {
            if !reachable[block.as_usize()] {
                continue;
            }
            for statement in &data.statements {
                let StatementKind::Assign(assignment) = &statement.kind else {
                    continue;
                };
                let (place, rvalue) = &**assignment;
                if !place.projection.is_empty() {
                    continue;
                }
                match rvalue {
                    Rvalue::Use(operand, _) => {
                        definitions[place.local.as_usize()].push(Definition::Use(operand.clone()))
                    }
                    Rvalue::Ref(_, _, source) => {
                        definitions[place.local.as_usize()].push(Definition::Ref(*source));
                    }
                    _ => {}
                }
            }

            if let TerminatorKind::Call {
                func,
                args,
                destination,
                ..
            } = &data.terminator().kind
                && destination.projection.is_empty()
                && let Some(callee) = callable_def_id(tcx, body, func)
            {
                definitions[destination.local.as_usize()].push(Definition::Call {
                    callee,
                    args: args.iter().map(|arg| arg.node.clone()).collect(),
                });
            }
        }

        Self {
            tcx,
            body,
            reachable,
            definitions,
            rust_type,
            depth,
        }
    }

    fn run(&self, direction: ConversionDirection) -> Result<LuaValueDomain, UnsupportedConversion> {
        match direction {
            ConversionDirection::IntoLua => self.analyze_into_lua(),
            ConversionDirection::FromLua => self.analyze_from_lua(),
        }
    }

    fn analyze_into_lua(&self) -> Result<LuaValueDomain, UnsupportedConversion> {
        let mut domains = Vec::new();
        let mut table_fields = Vec::new();
        let mut has_table = false;
        let mut unsupported = None;

        for (block, data) in self.body.basic_blocks.iter_enumerated() {
            if !self.reachable[block.as_usize()] {
                continue;
            }

            for statement in &data.statements {
                let StatementKind::Assign(assignment) = &statement.kind else {
                    continue;
                };
                let (_, Rvalue::Aggregate(kind, operands)) = &**assignment else {
                    continue;
                };
                let AggregateKind::Adt(adt_def_id, variant_index, ..) = &**kind else {
                    continue;
                };
                if !is_mlua_item(self.tcx, *adt_def_id, "Value") {
                    continue;
                }

                let variant = self.tcx.adt_def(*adt_def_id).variant(*variant_index);
                let operand = operands.raw.first();
                let domain = match variant.name.as_str() {
                    "Nil" => LuaValueDomain::Nil,
                    "Boolean" => self.scalar_or_literal(operand, LuaValueDomain::Boolean),
                    "Integer" => self.scalar_or_literal(operand, LuaValueDomain::Integer),
                    "Number" => self.scalar_or_literal(operand, LuaValueDomain::Number),
                    "String" => self.string_domain(operand),
                    "Table" => {
                        has_table = true;
                        continue;
                    }
                    "Function" => LuaValueDomain::Function,
                    "Thread" => LuaValueDomain::Thread,
                    "UserData" | "LightUserData" => LuaValueDomain::UserData(self.self_type_name()),
                    _ => {
                        unsupported = Some(UnsupportedConversion::UnsupportedValueVariant);
                        continue;
                    }
                };
                domains.push(domain);
            }

            let TerminatorKind::Call { func, args, .. } = &data.terminator().kind else {
                continue;
            };
            let Some(callee) = callable_def_id(self.tcx, self.body, func) else {
                continue;
            };
            if is_mlua_item(self.tcx, callee, "set") && args.len() >= 3 {
                let keys = self.string_literals(&args[1].node, 0);
                if keys.len() == 1 {
                    table_fields.push(LuaTableField {
                        name: keys[0].clone(),
                        value: domain_from_ty(
                            self.tcx,
                            args[2].node.ty(&self.body.local_decls, self.tcx),
                        ),
                    });
                } else {
                    unsupported = Some(UnsupportedConversion::DynamicTableKey);
                }
            } else if callee.is_local()
                && destination_contains_lua_value(self.tcx, self.body, data.terminator())
            {
                if self.depth >= 16 {
                    unsupported = Some(UnsupportedConversion::AnalysisBudgetExceeded);
                    continue;
                }
                let helper = self.tcx.optimized_mir(callee.expect_local());
                match Self::with_depth(self.tcx, helper, self.rust_type, self.depth + 1)
                    .analyze_into_lua()
                {
                    Ok(domain) => domains.push(domain),
                    Err(reason) => unsupported = Some(reason),
                }
            } else if !callee.is_local()
                && self.tcx.item_name(callee).as_str() != "from_residual"
                && destination_contains_lua_value(self.tcx, self.body, data.terminator())
            {
                unsupported = Some(UnsupportedConversion::UnavailableValueProducer);
            }
        }

        if let Some(reason) = unsupported {
            return Err(reason);
        }

        if has_table {
            table_fields.sort_by(|left, right| left.name.cmp(&right.name));
            table_fields
                .dedup_by(|left, right| left.name == right.name && left.value == right.value);
            domains.push(LuaValueDomain::Table(LuaTableShape {
                fields: table_fields,
                openness: TableOpenness::Closed,
            }));
        }

        join(domains).ok_or(UnsupportedConversion::MissingValueEvidence)
    }

    fn analyze_from_lua(&self) -> Result<LuaValueDomain, UnsupportedConversion> {
        let success_reachable = blocks_reaching_success(self.tcx, self.body, &self.reachable);
        let mut collector = InputVariantCollector {
            accepted_blocks: &success_reachable,
            variants: Vec::new(),
        };
        collector.visit_body(self.body);
        let mut domains = Vec::new();

        let input_ty = self.body.local_decls[Local::from_usize(1)].ty;
        let ty::Adt(value_def, _) = input_ty.kind() else {
            return Err(UnsupportedConversion::MissingValueEvidence);
        };
        if !is_mlua_item(self.tcx, value_def.did(), "Value") {
            return Err(UnsupportedConversion::MissingValueEvidence);
        }
        collector.variants.sort();
        collector.variants.dedup();
        let mut string_literals = Vec::new();
        let mut scalar_literals = Vec::new();
        let mut table_fields = Vec::new();
        for (block, data) in self.body.basic_blocks.iter_enumerated() {
            if !success_reachable[block.as_usize()] {
                continue;
            }
            match &data.terminator().kind {
                TerminatorKind::Call { func, args, .. } => {
                    let Some(callee) = callable_def_id(self.tcx, self.body, func) else {
                        continue;
                    };
                    if self.tcx.item_name(callee).as_str() == "eq" {
                        for argument in args {
                            string_literals.extend(self.string_literals(&argument.node, 0));
                            scalar_literals.extend(self.scalar_literals(&argument.node, 0));
                        }
                    } else if is_mlua_item(self.tcx, callee, "get") && args.len() >= 2 {
                        let keys = self.string_literals(&args[1].node, 0);
                        if keys.len() == 1
                            && let TerminatorKind::Call { destination, .. } =
                                &data.terminator().kind
                            && let Some(value_ty) =
                                result_value_ty(self.tcx, destination.ty(self.body, self.tcx).ty)
                        {
                            table_fields.push(LuaTableField {
                                name: keys[0].clone(),
                                value: domain_from_ty(self.tcx, value_ty),
                            });
                        }
                    }
                }
                TerminatorKind::SwitchInt { discr, targets }
                    if self.input_variant_for_operand(discr, 0).is_some() =>
                {
                    for (value, target) in targets.iter() {
                        if success_reachable[target.as_usize()]
                            && let Some(literal) = switch_literal(self.tcx, self.body, discr, value)
                        {
                            scalar_literals.push(literal);
                        }
                    }
                }
                _ => {}
            }
        }
        string_literals.sort();
        string_literals.dedup();
        scalar_literals.dedup();
        table_fields.sort_by(|left, right| left.name.cmp(&right.name));
        table_fields.dedup_by(|left, right| left.name == right.name && left.value == right.value);
        for index in collector.variants {
            let variant = value_def.variant(index).name.as_str();
            domains.push(match variant {
                "Nil" => LuaValueDomain::Nil,
                "Boolean" => literals_of_kind(&scalar_literals, LiteralKind::Boolean)
                    .unwrap_or(LuaValueDomain::Boolean),
                "Integer" => literals_of_kind(&scalar_literals, LiteralKind::Integer)
                    .unwrap_or(LuaValueDomain::Integer),
                "Number" => literals_of_kind(&scalar_literals, LiteralKind::Number)
                    .unwrap_or(LuaValueDomain::Number),
                "String" if !string_literals.is_empty() => join(
                    string_literals
                        .iter()
                        .cloned()
                        .map(|value| LuaValueDomain::Literal(LuaLiteral::String(value)))
                        .collect(),
                )
                .ok_or(UnsupportedConversion::MissingValueEvidence)?,
                "String" => LuaValueDomain::String,
                "Table" => LuaValueDomain::Table(LuaTableShape {
                    fields: table_fields.clone(),
                    openness: TableOpenness::Open,
                }),
                "Function" => LuaValueDomain::Function,
                "Thread" => LuaValueDomain::Thread,
                "UserData" | "LightUserData" => LuaValueDomain::UserData(self.self_type_name()),
                _ => continue,
            });
        }

        join(domains).ok_or(UnsupportedConversion::MissingValueEvidence)
    }

    fn self_type_name(&self) -> String {
        self.rust_type.to_owned()
    }

    fn scalar_or_literal(
        &self,
        operand: Option<&Operand<'tcx>>,
        scalar: LuaValueDomain,
    ) -> LuaValueDomain {
        let Some(operand) = operand else {
            return scalar;
        };
        let literals = self.scalar_literals(operand, 0);
        if literals.is_empty() {
            scalar
        } else {
            join(literals.into_iter().map(LuaValueDomain::Literal).collect()).unwrap_or(scalar)
        }
    }

    fn scalar_literals(&self, operand: &Operand<'tcx>, depth: usize) -> Vec<LuaLiteral> {
        if depth >= 32 {
            return Vec::new();
        }
        if let Some(literal) = literal_from_operand(self.tcx, self.body, operand) {
            return vec![literal];
        }
        let Some(place) = operand.place() else {
            return Vec::new();
        };
        let mut literals = Vec::new();
        for definition in &self.definitions[place.local.as_usize()] {
            match definition {
                Definition::Use(source) => {
                    literals.extend(self.scalar_literals(source, depth + 1));
                }
                Definition::Call { callee, args }
                    if self.tcx.item_name(*callee).as_str() == "branch" =>
                {
                    for argument in args {
                        literals.extend(self.scalar_literals(argument, depth + 1));
                    }
                }
                Definition::Call { callee, .. } if callee.is_local() => {
                    let helper = self.tcx.optimized_mir(callee.expect_local());
                    let helper_analyzer =
                        Self::with_depth(self.tcx, helper, self.rust_type, self.depth + 1);
                    for definition in &helper_analyzer.definitions[0] {
                        if let Definition::Use(source) = definition {
                            literals.extend(helper_analyzer.scalar_literals(source, depth + 1));
                        }
                    }
                }
                Definition::Ref(_) | Definition::Call { .. } => {}
            }
        }
        literals.dedup();
        literals
    }

    fn input_variant_for_operand(&self, operand: &Operand<'tcx>, depth: usize) -> Option<String> {
        if depth >= 32 {
            return None;
        }
        let place = operand.place()?;
        if place.local == Local::from_usize(1) {
            let value_ty = self.body.local_decls[place.local].ty;
            let ty::Adt(value_def, _) = value_ty.kind() else {
                return None;
            };
            for element in place.projection {
                if let PlaceElem::Downcast(_, variant) = element {
                    return Some(value_def.variant(variant).name.as_str().to_owned());
                }
            }
        }
        for definition in &self.definitions[place.local.as_usize()] {
            if let Definition::Use(source) = definition
                && let Some(variant) = self.input_variant_for_operand(source, depth + 1)
            {
                return Some(variant);
            }
        }
        None
    }

    fn string_domain(&self, operand: Option<&Operand<'tcx>>) -> LuaValueDomain {
        let Some(operand) = operand else {
            return LuaValueDomain::String;
        };
        let literals = self.string_literals(operand, 0);
        if literals.is_empty() {
            LuaValueDomain::String
        } else {
            join(
                literals
                    .into_iter()
                    .map(|value| LuaValueDomain::Literal(LuaLiteral::String(value)))
                    .collect(),
            )
            .unwrap_or(LuaValueDomain::String)
        }
    }

    fn string_literals(&self, operand: &Operand<'tcx>, depth: usize) -> Vec<String> {
        if depth >= 32 {
            return Vec::new();
        }
        if let Operand::Constant(constant) = operand {
            if let Some(value) = string_literal_from_operand(self.tcx, self.body, operand) {
                return vec![value];
            }
            if let rustc_middle::mir::Const::Unevaluated(unevaluated, _) = constant.const_
                && let Some(promoted) = unevaluated.promoted
            {
                let promoted_body = &self.tcx.promoted_mir(unevaluated.def)[promoted];
                let promoted_analyzer =
                    Self::with_depth(self.tcx, promoted_body, self.rust_type, self.depth + 1);
                let mut values = Vec::new();
                for definition in &promoted_analyzer.definitions[0] {
                    if let Definition::Use(source) = definition {
                        values.extend(promoted_analyzer.string_literals(source, depth + 1));
                    }
                }
                return values;
            }
        }
        let Some(place) = operand.place() else {
            return Vec::new();
        };

        let mut values = Vec::new();
        for definition in &self.definitions[place.local.as_usize()] {
            match definition {
                Definition::Use(source) => {
                    values.extend(self.string_literals(source, depth + 1));
                }
                Definition::Ref(source) => {
                    values.extend(self.string_literals(&Operand::Copy(*source), depth + 1));
                }
                Definition::Call { callee, args }
                    if is_mlua_item(self.tcx, *callee, "create_string") && args.len() >= 2 =>
                {
                    values.extend(self.string_literals(&args[1], depth + 1));
                }
                Definition::Call { callee, args }
                    if self.tcx.item_name(*callee).as_str() == "branch" =>
                {
                    for argument in args {
                        values.extend(self.string_literals(argument, depth + 1));
                    }
                }
                Definition::Call { callee, .. } if callee.is_local() => {
                    let helper = self.tcx.optimized_mir(callee.expect_local());
                    let helper_analyzer =
                        Self::with_depth(self.tcx, helper, self.rust_type, self.depth + 1);
                    for definition in &helper_analyzer.definitions[0] {
                        if let Definition::Use(source) = definition {
                            values.extend(helper_analyzer.string_literals(source, depth + 1));
                        }
                    }
                }
                Definition::Call { .. } => {}
            }
        }
        values.sort();
        values.dedup();
        values
    }
}

struct InputVariantCollector<'a> {
    accepted_blocks: &'a [bool],
    variants: Vec<rustc_abi::VariantIdx>,
}

impl<'tcx> Visitor<'tcx> for InputVariantCollector<'_> {
    fn visit_place(
        &mut self,
        place: &Place<'tcx>,
        context: rustc_middle::mir::visit::PlaceContext,
        location: Location,
    ) {
        if self.accepted_blocks[location.block.as_usize()] && place.local == Local::from_usize(1) {
            for element in place.projection {
                if let PlaceElem::Downcast(_, variant) = element {
                    self.variants.push(variant);
                }
            }
        }
        self.super_place(place, context, location);
    }
}

fn reachable_blocks(body: &Body<'_>) -> Vec<bool> {
    let mut reachable = vec![false; body.basic_blocks.len()];
    let mut queue = VecDeque::from([BasicBlock::from_usize(0)]);
    while let Some(block) = queue.pop_front() {
        if std::mem::replace(&mut reachable[block.as_usize()], true) {
            continue;
        }
        queue.extend(body.basic_blocks[block].terminator().successors());
    }
    reachable
}

fn blocks_reaching_success(tcx: TyCtxt<'_>, body: &Body<'_>, reachable: &[bool]) -> Vec<bool> {
    let mut reverse = vec![Vec::new(); body.basic_blocks.len()];
    let mut success = VecDeque::new();
    for (block, data) in body.basic_blocks.iter_enumerated() {
        if !reachable[block.as_usize()] {
            continue;
        }
        for successor in data.terminator().successors() {
            reverse[successor.as_usize()].push(block);
        }
        if data.statements.iter().any(|statement| {
            let StatementKind::Assign(assignment) = &statement.kind else {
                return false;
            };
            let (place, Rvalue::Aggregate(kind, _)) = &**assignment else {
                return false;
            };
            let AggregateKind::Adt(def_id, variant_index, ..) = &**kind else {
                return false;
            };
            place.local == Local::from_usize(0)
                && tcx.item_name(*def_id).as_str() == "Result"
                && tcx.adt_def(*def_id).variant(*variant_index).name.as_str() == "Ok"
        }) {
            success.push_back(block);
        }
    }

    let mut result = vec![false; body.basic_blocks.len()];
    while let Some(block) = success.pop_front() {
        if std::mem::replace(&mut result[block.as_usize()], true) {
            continue;
        }
        success.extend(reverse[block.as_usize()].iter().copied());
    }
    result
}

fn callable_def_id<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    operand: &Operand<'tcx>,
) -> Option<DefId> {
    match operand.ty(&body.local_decls, tcx).kind() {
        ty::FnDef(def_id, _) => Some(*def_id),
        _ => None,
    }
}

fn destination_contains_lua_value<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    terminator: &rustc_middle::mir::Terminator<'tcx>,
) -> bool {
    let TerminatorKind::Call { destination, .. } = &terminator.kind else {
        return false;
    };
    type_contains_lua_value(tcx, destination.ty(body, tcx).ty)
}

fn type_contains_lua_value(tcx: TyCtxt<'_>, ty: Ty<'_>) -> bool {
    match ty.kind() {
        ty::Adt(def, args) => {
            is_mlua_item(tcx, def.did(), "Value")
                || args
                    .types()
                    .any(|inner| type_contains_lua_value(tcx, inner))
        }
        ty::Ref(_, inner, _) => type_contains_lua_value(tcx, *inner),
        _ => false,
    }
}

fn result_value_ty<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> Option<Ty<'tcx>> {
    let ty::Adt(def, args) = ty.kind() else {
        return None;
    };
    (tcx.item_name(def.did()).as_str() == "Result")
        .then(|| args.types().next())
        .flatten()
}

fn is_mlua_item(tcx: TyCtxt<'_>, def_id: DefId, name: &str) -> bool {
    tcx.crate_name(def_id.krate).as_str() == "mlua" && tcx.item_name(def_id).as_str() == name
}

fn literal_from_operand<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    operand: &Operand<'tcx>,
) -> Option<LuaLiteral> {
    let Operand::Constant(constant) = operand else {
        return None;
    };
    let value = constant
        .const_
        .try_eval_scalar_int(tcx, body.typing_env(tcx))?;
    let ty = constant.const_.ty();
    match ty.kind() {
        ty::Bool => value.try_into().ok().map(LuaLiteral::Boolean),
        ty::Int(_) => {
            let size = tcx
                .layout_of(body.typing_env(tcx).as_query_input(ty))
                .ok()?
                .size;
            i64::try_from(value.to_int(size))
                .ok()
                .map(LuaLiteral::Integer)
        }
        ty::Uint(_) => {
            let size = tcx
                .layout_of(body.typing_env(tcx).as_query_input(ty))
                .ok()?
                .size;
            i64::try_from(value.to_bits(size))
                .ok()
                .map(LuaLiteral::Integer)
        }
        ty::Float(ty::FloatTy::F32) => Some(LuaLiteral::Number(
            f64::from(f32::from_bits(value.to_u32())).to_bits(),
        )),
        ty::Float(ty::FloatTy::F64) => Some(LuaLiteral::Number(value.to_u64())),
        _ => None,
    }
}

fn string_literal_from_operand<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    operand: &Operand<'tcx>,
) -> Option<String> {
    let Operand::Constant(constant) = operand else {
        return None;
    };
    if !matches!(constant.const_.ty().kind(), ty::Ref(_, inner, _) if matches!(inner.kind(), ty::Str))
    {
        return None;
    }
    let value = constant
        .const_
        .eval(tcx, body.typing_env(tcx), constant.span)
        .ok()?;
    let bytes = value.try_get_slice_bytes_for_diagnostics(tcx)?;
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

fn domain_from_ty(tcx: TyCtxt<'_>, ty: Ty<'_>) -> LuaValueDomain {
    match ty.kind() {
        ty::Bool => LuaValueDomain::Boolean,
        ty::Int(_) | ty::Uint(_) => LuaValueDomain::Integer,
        ty::Float(_) => LuaValueDomain::Number,
        ty::Str => LuaValueDomain::String,
        ty::Ref(_, inner, _) => domain_from_ty(tcx, *inner),
        ty::Adt(def, _) if tcx.item_name(def.did()).as_str() == "String" => LuaValueDomain::String,
        ty::Adt(def, _) => LuaValueDomain::UserData(tcx.def_path_str(def.did())),
        _ => LuaValueDomain::UserData(ty.to_string()),
    }
}

#[derive(Clone, Copy)]
enum LiteralKind {
    Boolean,
    Integer,
    Number,
}

fn literals_of_kind(literals: &[LuaLiteral], kind: LiteralKind) -> Option<LuaValueDomain> {
    join(
        literals
            .iter()
            .filter(|literal| {
                matches!(
                    (kind, literal),
                    (LiteralKind::Boolean, LuaLiteral::Boolean(_))
                        | (LiteralKind::Integer, LuaLiteral::Integer(_))
                        | (LiteralKind::Number, LuaLiteral::Number(_))
                )
            })
            .cloned()
            .map(LuaValueDomain::Literal)
            .collect(),
    )
}

fn switch_literal<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    operand: &Operand<'tcx>,
    value: u128,
) -> Option<LuaLiteral> {
    let ty = operand.ty(&body.local_decls, tcx);
    match ty.kind() {
        ty::Bool => Some(LuaLiteral::Boolean(value != 0)),
        ty::Int(_) => {
            let size = tcx
                .layout_of(body.typing_env(tcx).as_query_input(ty))
                .ok()?
                .size;
            let bits = rustc_middle::ty::ScalarInt::try_from_uint(value, size)?;
            i64::try_from(bits.to_int(size))
                .ok()
                .map(LuaLiteral::Integer)
        }
        ty::Uint(_) => i64::try_from(value).ok().map(LuaLiteral::Integer),
        _ => None,
    }
}

fn join(mut domains: Vec<LuaValueDomain>) -> Option<LuaValueDomain> {
    let mut unique = Vec::new();
    for domain in domains.drain(..) {
        match domain {
            LuaValueDomain::Union(items) => {
                for item in items {
                    if !unique.contains(&item) {
                        unique.push(item);
                    }
                }
            }
            item if !unique.contains(&item) => unique.push(item),
            _ => {}
        }
    }
    match unique.len() {
        0 => None,
        1 => unique.pop(),
        _ => Some(LuaValueDomain::Union(unique)),
    }
}
