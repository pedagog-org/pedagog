use std::collections::HashMap;

use regex::Regex;

use crate::recipe::primitives::{ParamDef, ParamVal};

/// Validates assignment overrides against platform param declarations and
/// produces a fully-resolved flat param map (all defaults applied).
pub fn resolve_params(
    decls: &HashMap<String, ParamDef>,
    overrides: &HashMap<String, ParamVal>,
) -> Result<HashMap<String, ParamVal>, String> {
    for key in overrides.keys() {
        if !decls.contains_key(key) {
            return Err(format!("unknown param {key:?}"));
        }
    }

    let mut out = HashMap::new();
    for (key, decl) in decls {
        let r#override = overrides.get(key);
        let val = resolve_one(key, decl, r#override)?;
        out.insert(key.clone(), val);
    }
    Ok(out)
}

fn resolve_one(key: &str, decl: &ParamDef, r#override: Option<&ParamVal>) -> Result<ParamVal, String> {
    match decl {
        ParamDef::Bool { default, .. } => {
            match r#override {
                Some(ParamVal::Bool(b)) => Ok(ParamVal::Bool(*b)),
                Some(other) => Err(format!("param {key:?}: expected bool, got {:?}", other.param_type())),
                None => default
                    .map(ParamVal::Bool)
                    .ok_or_else(|| format!("param {key:?} is required")),
            }
        }
        ParamDef::Int { default, .. } => {
            match r#override {
                Some(ParamVal::Int(i)) => Ok(ParamVal::Int(*i)),
                Some(other) => Err(format!("param {key:?}: expected int, got {:?}", other.param_type())),
                None => default
                    .map(ParamVal::Int)
                    .ok_or_else(|| format!("param {key:?} is required")),
            }
        }
        ParamDef::Str { default, regex, .. } => {
            let val = match r#override {
                Some(ParamVal::Str(s)) => ParamVal::Str(s.clone()),
                Some(other) => return Err(format!("param {key:?}: expected str, got {:?}", other.param_type())),
                None => default
                    .clone()
                    .map(ParamVal::Str)
                    .ok_or_else(|| format!("param {key:?} is required"))?,
            };
            if let (ParamVal::Str(s), Some(pattern)) = (&val, regex) {
                let re = Regex::new(pattern)
                    .map_err(|e| format!("param {key:?}: invalid regex {pattern:?}: {e}"))?;
                if !re.is_match(s) {
                    return Err(format!(
                        "param {key:?}: value {s:?} does not match regex {pattern:?}"
                    ));
                }
            }
            Ok(val)
        }
        ParamDef::List { default, .. } => {
            match r#override {
                Some(ParamVal::List(v)) => Ok(ParamVal::List(v.clone())),
                Some(other) => Err(format!("param {key:?}: expected list, got {:?}", other.param_type())),
                None => default
                    .clone()
                    .map(ParamVal::List)
                    .ok_or_else(|| format!("param {key:?} is required")),
            }
        }
        ParamDef::Map { properties, .. } => {
            let inner_overrides = match r#override {
                Some(ParamVal::Map(m)) => m.clone(),
                Some(other) => return Err(format!("param {key:?}: expected map, got {:?}", other.param_type())),
                None => HashMap::new(),
            };
            let resolved = resolve_params(properties, &inner_overrides)
                .map_err(|e| format!("param {key:?}.{e}"))?;
            Ok(ParamVal::Map(resolved))
        }
    }
}

/// Substitutes `{key.path}` and `{key.path:json}` tokens in `cmd`.
///
/// - Default format: bools/ints/strs as strings, lists as space-joined scalars.
/// - `:json` format: serde_json serialization of any `ParamVal`.
/// - Missing key or unsupported format returns `Err`.
pub fn interpolate(cmd: &str, params: &HashMap<String, ParamVal>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = cmd;

    loop {
        // Find the next `{` or `}}` — whichever comes first.
        let next_open = rest.find('{');
        let next_close2 = rest.find("}}");

        let advance_to = match (next_open, next_close2) {
            (None, None) => {
                out.push_str(rest);
                break;
            }
            (Some(o), Some(c)) if c < o => Err(c), // `}}` wins
            (None, Some(c)) => Err(c),              // only `}}`
            (Some(o), _) => Ok(o),                  // `{` wins (or no `}}`)
        };

        match advance_to {
            Err(c) => {
                // `}}` escape → literal `}`
                out.push_str(&rest[..c]);
                out.push('}');
                rest = &rest[c + 2..];
            }
            Ok(o) => {
                let is_shell_var = o > 0 && rest.as_bytes()[o - 1] == b'$';
                out.push_str(&rest[..o]);
                rest = &rest[o + 1..];

                // `{{` escape → literal `{`
                if rest.starts_with('{') {
                    out.push('{');
                    rest = &rest[1..];
                    continue;
                }

                // Shell variable `${...}` — pass through verbatim.
                if is_shell_var {
                    let close = rest
                        .find('}')
                        .ok_or_else(|| format!("unclosed '{{' in: {cmd}"))?;
                    out.push('{');
                    out.push_str(&rest[..close + 1]);
                    rest = &rest[close + 1..];
                    continue;
                }

                let close = rest
                    .find('}')
                    .ok_or_else(|| format!("unclosed '{{' in: {cmd}"))?;
                let inner = &rest[..close];
                rest = &rest[close + 1..];

                let (path, fmt) = match inner.find(':') {
                    Some(colon) => (&inner[..colon], Some(&inner[colon + 1..])),
                    None => (inner, None),
                };

                // `a+b` path syntax: union two or more lists of the same element type.
                let union_val: Option<ParamVal> = if path.contains('+') {
                    let mut combined: Vec<ParamVal> = Vec::new();
                    for p in path.split('+') {
                        let v = navigate(params, p)
                            .ok_or_else(|| format!("unknown param {p:?} in: {cmd}"))?;
                        match v {
                            ParamVal::List(items) => combined.extend(items.iter().cloned()),
                            _ => return Err(format!(
                                "param {p:?}: list union requires list, got {:?}",
                                v.param_type()
                            )),
                        }
                    }
                    Some(ParamVal::List(combined))
                } else {
                    None
                };

                let val = match &union_val {
                    Some(v) => v,
                    None => navigate(params, path)
                        .ok_or_else(|| format!("unknown param {path:?} in: {cmd}"))?,
                };

                let formatted = match fmt {
                    None => default_format(val, path)?,
                    Some("json") => serde_json::to_string(val)
                        .map_err(|e| format!("json serialization failed for {path:?}: {e}"))?,
                    Some(other) => return Err(format!("unknown format specifier {other:?} in: {cmd}")),
                };

                out.push_str(&formatted);
            }
        }
    }
    Ok(out)
}

fn navigate<'a>(params: &'a HashMap<String, ParamVal>, path: &str) -> Option<&'a ParamVal> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    let mut current = params.get(first)?;
    for part in parts {
        match current {
            ParamVal::Map(m) => current = m.get(part)?,
            _ => return None,
        }
    }
    Some(current)
}

fn default_format(val: &ParamVal, path: &str) -> Result<String, String> {
    match val {
        ParamVal::Bool(b) => Ok(b.to_string()),
        ParamVal::Int(i) => Ok(i.to_string()),
        ParamVal::Str(s) => Ok(s.clone()),
        ParamVal::List(items) => {
            let parts: Result<Vec<String>, String> = items
                .iter()
                .map(|v| match v {
                    ParamVal::Str(s) => Ok(s.clone()),
                    ParamVal::Bool(b) => Ok(b.to_string()),
                    ParamVal::Int(i) => Ok(i.to_string()),
                    _ => Err(format!("param {path:?}: list contains non-scalar value")),
                })
                .collect();
            Ok(parts?.join(" "))
        }
        ParamVal::Map(_) => Err(format!(
            "param {path:?}: cannot use default format for map; use {{{}:json}}",
            path
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bool_decl(default: Option<bool>) -> ParamDef {
        ParamDef::Bool { default, info: None }
    }

    fn str_decl(default: Option<&str>) -> ParamDef {
        ParamDef::Str { default: default.map(|s| s.to_owned()), info: None, regex: None }
    }

    fn list_decl(default: Option<Vec<ParamVal>>) -> ParamDef {
        ParamDef::List { default, info: None }
    }

    fn map_decl(properties: HashMap<String, ParamDef>) -> ParamDef {
        ParamDef::Map { properties, info: None }
    }

    // ---- resolve_params -----------------------------------------------------

    #[test]
    fn resolve_uses_defaults_when_no_overrides() {
        let mut decls = HashMap::new();
        decls.insert("terminal".to_owned(), bool_decl(Some(true)));
        decls.insert("label".to_owned(), str_decl(Some("default")));

        let result = resolve_params(&decls, &HashMap::new()).unwrap();

        assert!(matches!(result["terminal"], ParamVal::Bool(true)));
        assert!(matches!(result["label"], ParamVal::Str(ref s) if s == "default"));
    }

    #[test]
    fn resolve_applies_override() {
        let mut decls = HashMap::new();
        decls.insert("terminal".to_owned(), bool_decl(Some(true)));

        let mut overrides = HashMap::new();
        overrides.insert("terminal".to_owned(), ParamVal::Bool(false));

        let result = resolve_params(&decls, &overrides).unwrap();
        assert!(matches!(result["terminal"], ParamVal::Bool(false)));
    }

    #[test]
    fn resolve_error_on_unknown_key() {
        let decls = HashMap::new();
        let mut overrides = HashMap::new();
        overrides.insert("ghost".to_owned(), ParamVal::Bool(true));

        assert!(resolve_params(&decls, &overrides).is_err());
    }

    #[test]
    fn resolve_error_on_wrong_type() {
        let mut decls = HashMap::new();
        decls.insert("terminal".to_owned(), bool_decl(Some(true)));

        let mut overrides = HashMap::new();
        overrides.insert("terminal".to_owned(), ParamVal::Str("yes".to_owned()));

        assert!(resolve_params(&decls, &overrides).is_err());
    }

    #[test]
    fn resolve_error_on_missing_required() {
        let mut decls = HashMap::new();
        decls.insert("required".to_owned(), bool_decl(None));

        assert!(resolve_params(&decls, &HashMap::new()).is_err());
    }

    #[test]
    fn resolve_map_deep_merge() {
        let mut props = HashMap::new();
        props.insert("install".to_owned(), list_decl(Some(vec![])));
        props.insert("allow".to_owned(), list_decl(Some(vec![])));

        let mut decls = HashMap::new();
        decls.insert("extensions".to_owned(), map_decl(props));

        let mut inner = HashMap::new();
        inner.insert(
            "install".to_owned(),
            ParamVal::List(vec![ParamVal::Str("clangd".to_owned())]),
        );
        let mut overrides = HashMap::new();
        overrides.insert("extensions".to_owned(), ParamVal::Map(inner));

        let result = resolve_params(&decls, &overrides).unwrap();
        match &result["extensions"] {
            ParamVal::Map(m) => {
                assert!(matches!(&m["install"], ParamVal::List(v) if v.len() == 1));
                assert!(matches!(&m["allow"], ParamVal::List(v) if v.is_empty()));
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn resolve_map_all_defaults() {
        let mut props = HashMap::new();
        props.insert("install".to_owned(), list_decl(Some(vec![])));
        props.insert("allow".to_owned(), list_decl(Some(vec![])));

        let mut decls = HashMap::new();
        decls.insert("extensions".to_owned(), map_decl(props));

        let result = resolve_params(&decls, &HashMap::new()).unwrap();
        assert!(matches!(&result["extensions"], ParamVal::Map(_)));
    }

    // ---- interpolate --------------------------------------------------------

    fn params_from(pairs: &[(&str, ParamVal)]) -> HashMap<String, ParamVal> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn interpolate_bool() {
        let params = params_from(&[("terminal", ParamVal::Bool(true))]);
        assert_eq!(interpolate("val={terminal}", &params).unwrap(), "val=true");
    }

    #[test]
    fn interpolate_int() {
        let params = params_from(&[("port", ParamVal::Int(8080))]);
        assert_eq!(interpolate("port={port}", &params).unwrap(), "port=8080");
    }

    #[test]
    fn interpolate_str() {
        let params = params_from(&[("name", ParamVal::Str("alice".to_owned()))]);
        assert_eq!(interpolate("hello {name}", &params).unwrap(), "hello alice");
    }

    #[test]
    fn interpolate_list_space_joined() {
        let params = params_from(&[(
            "pkgs",
            ParamVal::List(vec![
                ParamVal::Str("gcc".to_owned()),
                ParamVal::Str("make".to_owned()),
            ]),
        )]);
        assert_eq!(interpolate("apt install {pkgs}", &params).unwrap(), "apt install gcc make");
    }

    #[test]
    fn interpolate_empty_list() {
        let params = params_from(&[("pkgs", ParamVal::List(vec![]))]);
        assert_eq!(interpolate("install {pkgs} done", &params).unwrap(), "install  done");
    }

    #[test]
    fn interpolate_json_list() {
        let params = params_from(&[(
            "ids",
            ParamVal::List(vec![ParamVal::Str("a.b".to_owned()), ParamVal::Str("c.d".to_owned())]),
        )]);
        assert_eq!(interpolate("{ids:json}", &params).unwrap(), r#"["a.b","c.d"]"#);
    }

    #[test]
    fn interpolate_json_map() {
        let mut inner = HashMap::new();
        inner.insert("k".to_owned(), ParamVal::Bool(true));
        let params = params_from(&[("m", ParamVal::Map(inner))]);
        let out = interpolate("{m:json}", &params).unwrap();
        assert!(out.contains("\"k\":true"));
    }

    #[test]
    fn interpolate_dot_nested() {
        let mut inner = HashMap::new();
        inner.insert(
            "install".to_owned(),
            ParamVal::List(vec![ParamVal::Str("clangd".to_owned())]),
        );
        let params = params_from(&[("extensions", ParamVal::Map(inner))]);
        assert_eq!(
            interpolate("{extensions.install}", &params).unwrap(),
            "clangd"
        );
    }

    #[test]
    fn interpolate_missing_key_errors() {
        let params = HashMap::new();
        assert!(interpolate("{ghost}", &params).is_err());
    }

    #[test]
    fn interpolate_map_default_format_errors() {
        let params = params_from(&[("m", ParamVal::Map(HashMap::new()))]);
        assert!(interpolate("{m}", &params).is_err());
    }

    #[test]
    fn interpolate_unknown_format_errors() {
        let params = params_from(&[("x", ParamVal::Bool(true))]);
        assert!(interpolate("{x:yaml}", &params).is_err());
    }

    #[test]
    fn interpolate_unclosed_brace_errors() {
        let params = HashMap::new();
        assert!(interpolate("hello {world", &params).is_err());
    }

    #[test]
    fn interpolate_no_tokens_passthrough() {
        let params = HashMap::new();
        assert_eq!(interpolate("apt-get install -y curl", &params).unwrap(), "apt-get install -y curl");
    }

    #[test]
    fn interpolate_list_union_json() {
        let params = params_from(&[
            ("a", ParamVal::List(vec![ParamVal::Str("x".into()), ParamVal::Str("y".into())])),
            ("b", ParamVal::List(vec![ParamVal::Str("z".into())])),
        ]);
        assert_eq!(interpolate("{a+b:json}", &params).unwrap(), r#"["x","y","z"]"#);
    }

    #[test]
    fn interpolate_list_union_empty() {
        let params = params_from(&[
            ("a", ParamVal::List(vec![ParamVal::Str("x".into())])),
            ("b", ParamVal::List(vec![])),
        ]);
        assert_eq!(interpolate("{a+b:json}", &params).unwrap(), r#"["x"]"#);
    }

    #[test]
    fn interpolate_list_union_non_list_errors() {
        let params = params_from(&[
            ("a", ParamVal::List(vec![])),
            ("b", ParamVal::Bool(true)),
        ]);
        assert!(interpolate("{a+b:json}", &params).is_err());
    }

    #[test]
    fn interpolate_list_union_default_format() {
        let params = params_from(&[
            ("a", ParamVal::List(vec![ParamVal::Str("gcc".into())])),
            ("b", ParamVal::List(vec![ParamVal::Str("make".into())])),
        ]);
        assert_eq!(interpolate("{a+b}", &params).unwrap(), "gcc make");
    }

    #[test]
    fn interpolate_list_union_three_operands() {
        let params = params_from(&[
            ("a", ParamVal::List(vec![ParamVal::Str("x".into())])),
            ("b", ParamVal::List(vec![ParamVal::Str("y".into())])),
            ("c", ParamVal::List(vec![ParamVal::Str("z".into()), ParamVal::Str("w".into())])),
        ]);
        assert_eq!(
            interpolate("{a+b+c:json}", &params).unwrap(),
            r#"["x","y","z","w"]"#
        );
    }

    #[test]
    fn interpolate_double_brace_escape() {
        let params = HashMap::new();
        // {{...}} → {...}
        assert_eq!(interpolate("{{\"key\": true}}", &params).unwrap(), "{\"key\": true}");
    }

    #[test]
    fn interpolate_double_brace_with_param() {
        let params = params_from(&[("terminal", ParamVal::Bool(false))]);
        // mix of escape and real token
        assert_eq!(
            interpolate("{{{terminal}}}", &params).unwrap(),
            "{false}"
        );
    }

    #[test]
    fn interpolate_shell_variable_passthrough() {
        // ${VAR} shell syntax must not be treated as a param token.
        let params = HashMap::new();
        assert_eq!(
            interpolate("curl ${URL}/file-${VERSION}.deb", &params).unwrap(),
            "curl ${URL}/file-${VERSION}.deb"
        );
    }

    #[test]
    fn interpolate_json_extensions_object() {
        let mut inner = HashMap::new();
        inner.insert("ext.id".to_owned(), ParamVal::Bool(true));
        let params = params_from(&[("allowed", ParamVal::Map(inner))]);
        let out = interpolate("{allowed:json}", &params).unwrap();
        assert!(out.contains("\"ext.id\":true"));
    }

    // ---- regex validation ---------------------------------------------------

    fn str_decl_with_regex(default: Option<&str>, regex: &str) -> ParamDef {
        ParamDef::Str {
            default: default.map(|s| s.to_owned()),
            info: None,
            regex: Some(regex.to_owned()),
        }
    }

    #[test]
    fn resolve_str_regex_accepts_matching_value() {
        let mut decls = HashMap::new();
        decls.insert("label".to_owned(), str_decl_with_regex(None, r"^[a-z]+$"));
        let mut overrides = HashMap::new();
        overrides.insert("label".to_owned(), ParamVal::Str("hello".to_owned()));
        assert!(resolve_params(&decls, &overrides).is_ok());
    }

    #[test]
    fn resolve_str_regex_rejects_non_matching_value() {
        let mut decls = HashMap::new();
        decls.insert("label".to_owned(), str_decl_with_regex(None, r"^[a-z]+$"));
        let mut overrides = HashMap::new();
        overrides.insert("label".to_owned(), ParamVal::Str("Hello123".to_owned()));
        assert!(resolve_params(&decls, &overrides).is_err());
    }

    #[test]
    fn resolve_str_regex_accepts_matching_default() {
        let mut decls = HashMap::new();
        decls.insert("label".to_owned(), str_decl_with_regex(Some("abc"), r"^[a-z]+$"));
        let result = resolve_params(&decls, &HashMap::new()).unwrap();
        assert!(matches!(&result["label"], ParamVal::Str(s) if s == "abc"));
    }

    #[test]
    fn resolve_str_regex_rejects_invalid_pattern() {
        let mut decls = HashMap::new();
        decls.insert("label".to_owned(), str_decl_with_regex(None, r"[invalid"));
        let mut overrides = HashMap::new();
        overrides.insert("label".to_owned(), ParamVal::Str("x".to_owned()));
        assert!(resolve_params(&decls, &overrides).is_err());
    }
}
