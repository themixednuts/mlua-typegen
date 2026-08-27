use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeTemplate {
    Nil,
    Boolean,
    Integer,
    Number,
    String,
    Table,
    Function,
    Thread,
    Literal(LuaLiteralTemplate),
    Named {
        name: String,
        arguments: Vec<TypeTemplate>,
    },
    Parameter(String),
    Array(Box<TypeTemplate>),
    Optional(Box<TypeTemplate>),
    Map {
        key: Box<TypeTemplate>,
        value: Box<TypeTemplate>,
    },
    Union(Vec<TypeTemplate>),
    FunctionSignature {
        parameters: Vec<TypeTemplate>,
        returns: Vec<TypeTemplate>,
    },
    Dynamic(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LuaLiteralTemplate {
    Boolean(bool),
    Integer(i64),
    Number(u64),
    String(String),
}

impl TypeTemplate {
    pub fn parse(source: &str) -> Result<Self, ExpressionError> {
        let mut parser = Parser::new(source);
        let expression = parser.parse_union()?;
        parser.skip_whitespace();
        if !parser.is_end() {
            return Err(parser.error("unexpected trailing input"));
        }
        Ok(expression)
    }

    pub fn referenced_parameters(&self, output: &mut Vec<String>) {
        match self {
            Self::Parameter(name) => {
                if !output.contains(name) {
                    output.push(name.clone());
                }
            }
            Self::Named { arguments, .. } | Self::Union(arguments) => {
                for argument in arguments {
                    argument.referenced_parameters(output);
                }
            }
            Self::Array(inner) | Self::Optional(inner) => inner.referenced_parameters(output),
            Self::Map { key, value } => {
                key.referenced_parameters(output);
                value.referenced_parameters(output);
            }
            Self::FunctionSignature {
                parameters,
                returns,
            } => {
                for ty in parameters.iter().chain(returns) {
                    ty.referenced_parameters(output);
                }
            }
            Self::Nil
            | Self::Boolean
            | Self::Integer
            | Self::Number
            | Self::String
            | Self::Table
            | Self::Function
            | Self::Thread
            | Self::Literal(_)
            | Self::Dynamic(_) => {}
        }
    }

    pub fn instantiate(
        &self,
        parameters: &BTreeMap<String, TypeTemplate>,
    ) -> Result<TypeTemplate, InstantiateError> {
        Ok(match self {
            Self::Nil => Self::Nil,
            Self::Boolean => Self::Boolean,
            Self::Integer => Self::Integer,
            Self::Number => Self::Number,
            Self::String => Self::String,
            Self::Table => Self::Table,
            Self::Function => Self::Function,
            Self::Thread => Self::Thread,
            Self::Literal(value) => Self::Literal(value.clone()),
            Self::Named { name, arguments } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| argument.instantiate(parameters))
                    .collect::<Result<Vec<_>, _>>()?;
                Self::Named {
                    name: name.clone(),
                    arguments,
                }
            }
            Self::Parameter(name) => parameters
                .get(name)
                .cloned()
                .ok_or_else(|| InstantiateError::MissingParameter(name.clone()))?,
            Self::Array(inner) => Self::Array(Box::new(inner.instantiate(parameters)?)),
            Self::Optional(inner) => Self::Optional(Box::new(inner.instantiate(parameters)?)),
            Self::Map { key, value } => Self::Map {
                key: Box::new(key.instantiate(parameters)?),
                value: Box::new(value.instantiate(parameters)?),
            },
            Self::Union(types) => Self::Union(
                types
                    .iter()
                    .map(|ty| ty.instantiate(parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::FunctionSignature {
                parameters: inputs,
                returns,
            } => Self::FunctionSignature {
                parameters: inputs
                    .iter()
                    .map(|ty| ty.instantiate(parameters))
                    .collect::<Result<Vec<_>, _>>()?,
                returns: returns
                    .iter()
                    .map(|ty| ty.instantiate(parameters))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            Self::Dynamic(reason) => Self::Dynamic(reason.clone()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionError {
    offset: usize,
    message: &'static str,
}

impl ExpressionError {
    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for ExpressionError {}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum InstantiateError {
    #[error("mapping expression references unbound parameter `${0}`")]
    MissingParameter(String),
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, offset: 0 }
    }

    fn parse_union(&mut self) -> Result<TypeTemplate, ExpressionError> {
        let mut members = vec![self.parse_postfix()?];
        loop {
            self.skip_whitespace();
            if !self.consume("|") {
                break;
            }
            members.push(self.parse_postfix()?);
        }
        Ok(if members.len() == 1 {
            members.pop().expect("one union member")
        } else {
            TypeTemplate::Union(members)
        })
    }

    fn parse_postfix(&mut self) -> Result<TypeTemplate, ExpressionError> {
        let mut expression = self.parse_atom()?;
        loop {
            self.skip_whitespace();
            if self.consume("[]") {
                expression = TypeTemplate::Array(Box::new(expression));
            } else if self.consume("?") {
                expression = TypeTemplate::Optional(Box::new(expression));
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn parse_atom(&mut self) -> Result<TypeTemplate, ExpressionError> {
        self.skip_whitespace();
        if self.consume("(") {
            let expression = self.parse_union()?;
            self.expect(")")?;
            return Ok(expression);
        }
        if self.peek() == Some('"') {
            return self
                .parse_string()
                .map(|value| TypeTemplate::Literal(LuaLiteralTemplate::String(value)));
        }
        if self
            .peek()
            .is_some_and(|ch| ch == '-' || ch.is_ascii_digit())
        {
            return self.parse_number();
        }
        if self.consume("$") {
            return self.parse_identifier().map(TypeTemplate::Parameter);
        }

        let token_start = self.offset;
        let identifier = self.parse_identifier()?;
        match identifier.as_str() {
            "nil" => Ok(TypeTemplate::Nil),
            "boolean" => Ok(TypeTemplate::Boolean),
            "integer" => Ok(TypeTemplate::Integer),
            "number" => Ok(TypeTemplate::Number),
            "string" => Ok(TypeTemplate::String),
            "table" => Ok(TypeTemplate::Table),
            "function" => Ok(TypeTemplate::Function),
            "thread" => Ok(TypeTemplate::Thread),
            "true" => Ok(TypeTemplate::Literal(LuaLiteralTemplate::Boolean(true))),
            "false" => Ok(TypeTemplate::Literal(LuaLiteralTemplate::Boolean(false))),
            "any" => Err(ExpressionError {
                offset: token_start,
                message: "bare `any` is forbidden; use `dynamic(\"reason\")`",
            }),
            "dynamic" => self.parse_dynamic(),
            "map" => self.parse_map(),
            "fun" => self.parse_function(),
            _ => {
                let arguments = if self.consume("<") {
                    let values = self.parse_list(">")?;
                    if values.is_empty() {
                        return Err(self.error("named type arguments cannot be empty"));
                    }
                    values
                } else {
                    Vec::new()
                };
                Ok(TypeTemplate::Named {
                    name: identifier,
                    arguments,
                })
            }
        }
    }

    fn parse_dynamic(&mut self) -> Result<TypeTemplate, ExpressionError> {
        self.expect("(")?;
        let reason = self.parse_string()?;
        if reason.trim().is_empty() {
            return Err(self.error("dynamic reason cannot be empty"));
        }
        self.expect(")")?;
        Ok(TypeTemplate::Dynamic(reason))
    }

    fn parse_number(&mut self) -> Result<TypeTemplate, ExpressionError> {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E'))
        {
            self.offset += self.peek().expect("peeked number character").len_utf8();
        }
        let token = &self.source[start..self.offset];
        if !token.contains(['.', 'e', 'E']) {
            return token
                .parse::<i64>()
                .map(|value| TypeTemplate::Literal(LuaLiteralTemplate::Integer(value)))
                .map_err(|_| self.error("invalid integer literal"));
        }
        token
            .parse::<f64>()
            .map(|value| TypeTemplate::Literal(LuaLiteralTemplate::Number(value.to_bits())))
            .map_err(|_| self.error("invalid number literal"))
    }

    fn parse_map(&mut self) -> Result<TypeTemplate, ExpressionError> {
        self.expect("<")?;
        let key = self.parse_union()?;
        self.expect(",")?;
        let value = self.parse_union()?;
        self.expect(">")?;
        Ok(TypeTemplate::Map {
            key: Box::new(key),
            value: Box::new(value),
        })
    }

    fn parse_function(&mut self) -> Result<TypeTemplate, ExpressionError> {
        self.expect("(")?;
        let parameters = self.parse_list(")")?;
        self.skip_whitespace();
        let returns = if self.consume(":") {
            self.skip_whitespace();
            if self.consume("(") {
                self.parse_list(")")?
            } else {
                vec![self.parse_union()?]
            }
        } else {
            Vec::new()
        };
        Ok(TypeTemplate::FunctionSignature {
            parameters,
            returns,
        })
    }

    fn parse_list(&mut self, terminator: &str) -> Result<Vec<TypeTemplate>, ExpressionError> {
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.consume(terminator) {
            return Ok(values);
        }
        loop {
            values.push(self.parse_union()?);
            self.skip_whitespace();
            if self.consume(terminator) {
                return Ok(values);
            }
            self.expect(",")?;
        }
    }

    fn parse_identifier(&mut self) -> Result<String, ExpressionError> {
        self.skip_whitespace();
        let start = self.offset;
        while let Some(ch) = self.peek() {
            if ch.is_alphanumeric() || matches!(ch, '_' | '.' | ':') {
                self.offset += ch.len_utf8();
            } else {
                break;
            }
        }
        if self.offset == start {
            Err(self.error("expected a type name"))
        } else {
            Ok(self.source[start..self.offset].to_owned())
        }
    }

    fn parse_string(&mut self) -> Result<String, ExpressionError> {
        self.skip_whitespace();
        let start = self.offset;
        if !self.consume("\"") {
            return Err(self.error("expected a quoted string"));
        }
        let mut escaped = false;
        while let Some(ch) = self.peek() {
            self.offset += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return serde_json::from_str(&self.source[start..self.offset])
                    .map_err(|_| self.error("invalid quoted string"));
            }
        }
        Err(self.error("unterminated quoted string"))
    }

    fn expect(&mut self, expected: &str) -> Result<(), ExpressionError> {
        self.skip_whitespace();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected token"))
        }
    }

    fn consume(&mut self, expected: &str) -> bool {
        if self.source[self.offset..].starts_with(expected) {
            self.offset += expected.len();
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.offset += self.peek().expect("peeked character").len_utf8();
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn is_end(&self) -> bool {
        self.offset == self.source.len()
    }

    fn error(&self, message: &'static str) -> ExpressionError {
        ExpressionError {
            offset: self.offset,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_composed_target_neutral_expression() {
        let expression = TypeTemplate::parse(
            "fun($T, map<string, Page<$U>>): (\"ok\" | nil, dynamic(\"opaque payload\"))",
        )
        .unwrap();
        let mut parameters = Vec::new();
        expression.referenced_parameters(&mut parameters);
        assert_eq!(parameters, ["T", "U"]);
    }

    #[test]
    fn rejects_bare_any() {
        let error = TypeTemplate::parse("any").unwrap_err();
        assert!(error.to_string().contains("dynamic"));
    }

    #[test]
    fn rejects_empty_dynamic_reason() {
        let error = TypeTemplate::parse("dynamic(\"  \")").unwrap_err();
        assert!(error.to_string().contains("cannot be empty"));
    }

    #[test]
    fn parses_integer_and_floating_point_literals() {
        assert_eq!(
            TypeTemplate::parse("-42").unwrap(),
            TypeTemplate::Literal(LuaLiteralTemplate::Integer(-42))
        );
        assert_eq!(
            TypeTemplate::parse("1.5").unwrap(),
            TypeTemplate::Literal(LuaLiteralTemplate::Number(1.5_f64.to_bits()))
        );
    }
}
