use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use zutai_eval::{TlcSession, Value, thunk::ThunkState};
use zutai_semantic::{Analysis, RecordedAnalysis};
use zutai_thir::{ImportKey, ThirDeclKind, ThirFile, TypeId, TypeKind};

const MAX_DEPTH: usize = 256;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let (Some(root), Some(source)) = (args.next(), args.next()) else {
        eprintln!("usage: system-spec-source CONTRACTS_ROOT SOURCE [DERIVED_OUTPUT ...]");
        std::process::exit(2);
    };
    let source = PathBuf::from(source);
    let label = source.display().to_string();
    let forbidden = args.map(PathBuf::from).collect::<Vec<_>>();
    let result = std::thread::Builder::new()
        .name("system-spec-source".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || render_source(Path::new(&root), &source, &forbidden))
        .map_err(|error| error.to_string())
        .and_then(|worker| worker.join().map_err(|_| "compiler worker panicked".into()))
        .and_then(|result| result);
    match result {
        Ok(output) => {
            if let Err(error) = std::io::stdout().lock().write_all(output.as_bytes()) {
                eprintln!("{label}: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{label}: {error}");
            std::process::exit(1);
        }
    }
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn render_source(root: &Path, source: &Path, forbidden: &[PathBuf]) -> Result<String, String> {
    let root = canonical(root)?;
    let source = canonical(source)?;
    if !source.starts_with(&root) {
        return Err("source escapes contracts root".into());
    }
    if source.extension().and_then(|extension| extension.to_str()) != Some("zt") {
        return Err("computed source must have .zt extension".into());
    }
    let forbidden = forbidden
        .iter()
        .map(|path| {
            if path.exists() {
                canonical(path)
            } else {
                let parent = path.parent().ok_or("derived output has no parent")?;
                Ok(canonical(parent)?.join(path.file_name().ok_or("derived output has no name")?))
            }
        })
        .collect::<Result<HashSet<_>, String>>()?;
    let recorded = zutai_semantic::analyze_path_recording_with_root(&source, &root)
        .map_err(|error| error.to_string())?;
    check_paths(&recorded, &root, &forbidden)?;
    check_analysis(&recorded.analysis, &mut HashSet::new())?;
    let thir =
        zutai_eval::check_well_typed(&recorded.analysis).map_err(|error| error.to_string())?;
    check_output_type(
        thir,
        thir.expr_arena[thir.final_expr].ty,
        &mut HashSet::new(),
        0,
    )?;
    // Evaluate exactly the checked graph; no second path read or analysis is allowed.
    let session =
        TlcSession::from_analysis(&recorded.analysis).map_err(|error| error.to_string())?;
    let value = session.entry().map_err(|error| error.to_string())?;
    let mut output = String::new();
    render_value(&value, 0, &mut output)?;
    output.push('\n');
    Ok(output)
}

fn check_paths(
    recorded: &RecordedAnalysis,
    root: &Path,
    forbidden: &HashSet<PathBuf>,
) -> Result<(), String> {
    // Package identities are synthetic; their recorded filesystem paths still
    // have to obey the same contract-only boundary as quoted relative imports.
    for path in recorded.source_paths.values() {
        let path = canonical(path)?;
        if !path.starts_with(root) {
            return Err(format!("import escapes contracts root: {}", path.display()));
        }
        if forbidden.contains(&path) {
            return Err(format!(
                "import of derived output is forbidden: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn check_analysis(analysis: &Analysis, seen: &mut HashSet<*const Analysis>) -> Result<(), String> {
    if !seen.insert(analysis) {
        return Ok(());
    }
    let label = analysis
        .source_path
        .as_deref()
        .unwrap_or(Path::new("<unknown>"))
        .display();
    if !analysis.diagnostics.is_empty() {
        return Err(format!("{label}: diagnostics: {:?}", analysis.diagnostics));
    }
    if analysis.effectful_program().is_some() {
        return Err(format!(
            "{label}: effects are forbidden in computed sources"
        ));
    }
    for key in analysis.import_sites.keys() {
        if matches!(key, ImportKey::Path(parts) if parts.first().is_some_and(|part| part == "stdlib"))
        {
            return Err(format!("{label}: explicit stdlib imports are forbidden"));
        }
    }
    for child in analysis.import_modules.values() {
        check_analysis(child, seen)?;
    }
    Ok(())
}

fn check_output_type(
    file: &ThirFile,
    ty: TypeId,
    seen: &mut HashSet<TypeId>,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("result type exceeds nesting limit".into());
    }
    if !seen.insert(ty) {
        return Ok(());
    }
    // TLC erases Type values to Nothing, including record fields. Reject them
    // before evaluation rather than silently dropping a non-data result field.
    match &file.type_arena[ty.0 as usize].kind {
        TypeKind::Bool
        | TypeKind::True
        | TypeKind::False
        | TypeKind::Int
        | TypeKind::FixedNum(_)
        | TypeKind::Text
        | TypeKind::InferVar(_) => Ok(()),
        TypeKind::List(inner) => check_output_type(file, *inner, seen, depth + 1),
        TypeKind::Record(fields, _) => {
            for field in fields {
                check_output_type(file, field.ty, seen, depth + 1)?;
            }
            Ok(())
        }
        TypeKind::Alias(binding) => {
            let body = file.decl_arena.iter().find_map(|(_, declaration)| {
                if declaration.binding == *binding
                    && let ThirDeclKind::TypeAlias { ty, .. } = declaration.kind
                {
                    Some(ty)
                } else {
                    None
                }
            });
            check_output_type(
                file,
                body.ok_or("unresolved result type alias")?,
                seen,
                depth + 1,
            )
        }
        kind => Err(format!(
            "result must contain only Bool, Int, Text, List, or Record; found {kind:?}"
        )),
    }
}

fn quoted(text: &str) -> String {
    let json = serde_json::to_string(text).expect("text serialization is infallible");
    let mut output = String::new();
    for character in json.chars() {
        if character.is_ascii() && character != '\u{7f}' {
            output.push(character);
        } else {
            for unit in character.encode_utf16(&mut [0; 2]) {
                write!(output, "\\u{unit:04x}").expect("String write is infallible");
            }
        }
    }
    output
}

fn render_thunk(
    thunk: &zutai_eval::Thunk,
    depth: usize,
    output: &mut String,
) -> Result<(), String> {
    match &*thunk.0.borrow() {
        ThunkState::Forced(value) => render_value(value, depth, output),
        _ => Err("result contains an unforced value".into()),
    }
}

fn render_value(value: &Value, depth: usize, output: &mut String) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("result exceeds nesting limit".into());
    }
    let padding = "  ".repeat(depth);
    match value {
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Int(value) => write!(output, "{value}").expect("String write is infallible"),
        Value::Text(value) => output.push_str(&quoted(value)),
        Value::List(values) if values.is_empty() => output.push_str("[]"),
        Value::List(values) => {
            output.push_str("[\n");
            for value in values.iter() {
                write!(output, "{padding}  ").expect("String write is infallible");
                render_thunk(value, depth + 1, output)?;
                output.push_str(";\n");
            }
            write!(output, "{padding}]").expect("String write is infallible");
        }
        Value::Record(fields) => {
            output.push_str("{\n");
            // The evaluator's Vec preserves source/update order; Display sorts.
            for (name, value) in fields.iter() {
                write!(output, "{padding}  {name} = ").expect("String write is infallible");
                render_thunk(value, depth + 1, output)?;
                output.push_str(";\n");
            }
            write!(output, "{padding}}}").expect("String write is infallible");
        }
        _ => return Err("result must contain only Bool, Int, Text, List, or Record".into()),
    }
    if output.len() > MAX_OUTPUT_BYTES {
        return Err("rendered result exceeds 16 MiB limit".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use zutai_eval::Thunk;

    #[test]
    fn rendering_preserves_order_and_python_ascii_escaping() {
        let value = Value::Record(Rc::new(vec![
            ("z".into(), Thunk::ready(Value::Int(3))),
            ("a".into(), Thunk::ready(Value::Text("雪😀\n\t\"\\".into()))),
            ("empty".into(), Thunk::ready(Value::Record(Rc::new(vec![])))),
            (
                "list".into(),
                Thunk::ready(Value::List(Rc::from(vec![Thunk::ready(Value::Bool(true))]))),
            ),
        ]));
        let mut output = String::new();
        render_value(&value, 0, &mut output).unwrap();
        assert_eq!(
            output,
            "{\n  z = 3;\n  a = \"\\u96ea\\ud83d\\ude00\\n\\t\\\"\\\\\";\n  empty = {\n  };\n  list = [\n    true;\n  ];\n}"
        );
    }

    #[test]
    fn ascii_escaping_includes_delete_and_non_bmp_text() {
        assert_eq!(quoted("\u{7f}\u{80}😀"), "\"\\u007f\\u0080\\ud83d\\ude00\"");
    }

    #[test]
    fn output_limits_are_enforced() {
        assert!(render_value(&Value::Bool(true), MAX_DEPTH + 1, &mut String::new()).is_err());
        let mut output = " ".repeat(MAX_OUTPUT_BYTES);
        assert!(render_value(&Value::Bool(true), 0, &mut output).is_err());
    }

    #[test]
    fn non_data_is_not_rendered() {
        for value in [Value::Nothing, Value::Float(1.5), Value::Atom("tag".into())] {
            assert!(render_value(&value, 0, &mut String::new()).is_err());
        }
    }
}
