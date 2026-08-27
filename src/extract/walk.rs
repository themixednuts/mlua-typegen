use rustc_hir as hir;

/// Walk statements in a loop body block, calling `$visitor` on each expression.
/// Uses a macro to avoid lifetime issues with `TyCtxt`'s invariant lifetime parameter.
macro_rules! walk_loop_body {
    ($block:expr, $visitor:expr) => {
        for stmt in $block.stmts {
            match &stmt.kind {
                hir::StmtKind::Semi(e) | hir::StmtKind::Expr(e) => {
                    $visitor(e);
                }
                _ => {}
            }
        }
        if let Some(e) = $block.expr {
            $visitor(e);
        }
    };
}

pub(super) fn visit_structural_expr_children<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    mut visit: impl FnMut(&'tcx hir::Expr<'tcx>),
) {
    match &expr.kind {
        hir::ExprKind::Block(block, _) => {
            for stmt in block.stmts {
                match &stmt.kind {
                    hir::StmtKind::Semi(expr) | hir::StmtKind::Expr(expr) => visit(expr),
                    hir::StmtKind::Let(local) => {
                        if let Some(init) = local.init {
                            visit(init);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(expr) = block.expr {
                visit(expr);
            }
        }
        hir::ExprKind::If(cond, then_expr, else_expr) => {
            visit(cond);
            visit(then_expr);
            if let Some(else_expr) = else_expr {
                visit(else_expr);
            }
        }
        hir::ExprKind::Match(scrutinee, arms, _) => {
            visit(scrutinee);
            for arm in *arms {
                visit(arm.body);
            }
        }
        hir::ExprKind::Loop(block, _, _, _) => {
            walk_loop_body!(block, &mut visit);
        }
        hir::ExprKind::DropTemps(inner)
        | hir::ExprKind::Use(inner, _)
        | hir::ExprKind::Unary(_, inner)
        | hir::ExprKind::Cast(inner, _)
        | hir::ExprKind::Type(inner, _)
        | hir::ExprKind::Field(inner, _)
        | hir::ExprKind::AddrOf(_, _, inner)
        | hir::ExprKind::Become(inner)
        | hir::ExprKind::Yield(inner, _)
        | hir::ExprKind::UnsafeBinderCast(_, inner, _) => visit(inner),
        hir::ExprKind::Assign(lhs, rhs, _) | hir::ExprKind::AssignOp(_, lhs, rhs) => {
            visit(lhs);
            visit(rhs);
        }
        hir::ExprKind::Index(lhs, rhs, _) | hir::ExprKind::Binary(_, lhs, rhs) => {
            visit(lhs);
            visit(rhs);
        }
        hir::ExprKind::Array(exprs) | hir::ExprKind::Tup(exprs) => {
            for expr in *exprs {
                visit(expr);
            }
        }
        hir::ExprKind::Repeat(value, _) => visit(value),
        hir::ExprKind::Ret(value) | hir::ExprKind::Break(_, value) => {
            if let Some(value) = value {
                visit(value);
            }
        }
        hir::ExprKind::Struct(_, fields, tail) => {
            for field in *fields {
                visit(field.expr);
            }
            if let hir::StructTailExpr::Base(base) = tail {
                visit(base);
            }
        }
        hir::ExprKind::Let(let_expr) => visit(let_expr.init),
        _ => {}
    }
}

pub(super) fn visit_recursive_expr_children<'tcx>(
    expr: &'tcx hir::Expr<'tcx>,
    mut visit: impl FnMut(&'tcx hir::Expr<'tcx>),
) {
    match &expr.kind {
        hir::ExprKind::Call(callee, args) => {
            visit(callee);
            for arg in *args {
                visit(arg);
            }
        }
        hir::ExprKind::MethodCall(_, receiver, args, _) => {
            visit(receiver);
            for arg in *args {
                visit(arg);
            }
        }
        _ => visit_structural_expr_children(expr, visit),
    }
}

pub(super) fn find_in_recursive_expr_children<'tcx, T>(
    expr: &'tcx hir::Expr<'tcx>,
    mut visit: impl FnMut(&'tcx hir::Expr<'tcx>) -> Option<T>,
) -> Option<T> {
    let mut found = None;
    visit_recursive_expr_children(expr, |child| {
        if found.is_none() {
            found = visit(child);
        }
    });
    found
}
