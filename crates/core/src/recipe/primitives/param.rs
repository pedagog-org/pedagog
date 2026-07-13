use std::collections::HashMap;

use serde::Deserialize;

/// Discriminant-only counterpart to `ParamVal` — used in `ParamDef` declarations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Bool,
    Int,
    Str,
    List,
    Map,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum ParamVal {
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<ParamVal>),
    Map(HashMap<String, ParamVal>),
}

impl ParamVal {
    pub fn param_type(&self) -> ParamType {
        match self {
            ParamVal::Bool(_) => ParamType::Bool,
            ParamVal::Int(_) => ParamType::Int,
            ParamVal::Str(_) => ParamType::Str,
            ParamVal::List(_) => ParamType::List,
            ParamVal::Map(_) => ParamType::Map,
        }
    }
}

/// Typed parameter declaration used in platform/toolchain recipe `params:` blocks.
/// The `type` field is the serde tag; defaults are typed to their variant.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ParamDef {
    Bool { default: Option<bool>,          #[serde(default)] info: Option<String> },
    Int  { default: Option<i64>,           #[serde(default)] info: Option<String> },
    Str  { default: Option<String>,        #[serde(default)] info: Option<String>, #[serde(default)] regex: Option<String> },
    List { default: Option<Vec<ParamVal>>, #[serde(default)] info: Option<String> },
    /// Map params carry recursive typed sub-property declarations.
    /// Their effective default is derived from sub-properties' defaults.
    Map  { properties: HashMap<String, ParamDef>, #[serde(default)] info: Option<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_val_param_type() {
        assert_eq!(ParamVal::Bool(true).param_type(), ParamType::Bool);
        assert_eq!(ParamVal::Int(42).param_type(), ParamType::Int);
        assert_eq!(ParamVal::Str("hi".into()).param_type(), ParamType::Str);
        assert_eq!(ParamVal::List(vec![]).param_type(), ParamType::List);
        assert_eq!(ParamVal::Map(HashMap::new()).param_type(), ParamType::Map);
    }

    #[test]
    fn paramdef_bool() {
        let yaml = "type: bool\ndefault: true\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::Bool { default: Some(true), .. }));
    }

    #[test]
    fn paramdef_bool_no_default() {
        let yaml = "type: bool\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::Bool { default: None, .. }));
    }

    #[test]
    fn paramdef_int() {
        let yaml = "type: int\ndefault: 42\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::Int { default: Some(42), .. }));
    }

    #[test]
    fn paramdef_str() {
        let yaml = "type: str\ndefault: hello\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::Str { default: Some(ref s), .. } if s == "hello"));
    }

    #[test]
    fn paramdef_list_empty_default() {
        let yaml = "type: list\ndefault: []\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::List { default: Some(ref v), .. } if v.is_empty()));
    }

    #[test]
    fn paramdef_map_with_properties() {
        let yaml = "type: map\nproperties:\n  install:\n    type: list\n    default: []\n  allow:\n    type: list\n    default: []\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        match def {
            ParamDef::Map { properties, .. } => {
                assert!(properties.contains_key("install"));
                assert!(properties.contains_key("allow"));
                assert!(matches!(properties["install"], ParamDef::List { .. }));
            }
            _ => panic!("expected Map variant"),
        }
    }

    #[test]
    fn paramdef_unknown_type_errors() {
        let yaml = "type: foobar\ndefault: true\n";
        assert!(serde_yaml::from_str::<ParamDef>(yaml).is_err());
    }

    #[test]
    fn paramdef_info_roundtrip() {
        let yaml = "type: bool\ndefault: true\ninfo: 'enable terminal'\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::Bool { info: Some(ref s), .. } if s == "enable terminal"));
    }

    #[test]
    fn paramdef_str_regex_field() {
        let yaml = "type: str\nregex: '^[a-z]+$'\n";
        let def: ParamDef = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(def, ParamDef::Str { regex: Some(ref r), .. } if r == "^[a-z]+$"));
    }
}
