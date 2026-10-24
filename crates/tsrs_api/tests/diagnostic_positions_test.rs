mod common;
use common::*;
use tsrs_core::json::Value;

#[test]
fn diagnostic_positions_preserve_utf16_across_unicode_and_ecma_line_breaks() {
    let dir = TempDir::new("diagnostic-positions");
    let source = "/* é🧪 */function first(alpha){return alpha;}\r\n/* 🧪 */function second(beta){return beta;}\r/* é */function third(gamma){return gamma;}\u{2028}function fourth(delta){return delta;}\u{2029}function fifth(epsilon){return epsilon;}";
    let file = dir.write("a.js", source);
    let session = session(&dir.dir(), false);
    let snapshot = call(
        &session,
        "createSnapshot",
        &format!(
            "{{\"createPrograms\":[{{\"rootFiles\":[{}],\"compilerOptions\":{{\"allowJs\":true,\"checkJs\":true,\"strict\":true,\"noEmit\":true}}}}]}}",
            quote(&file)
        ),
    );
    let snapshot_id = get(&snapshot, "snapshot");
    let project = str_of(get(&snapshot, "operation.createdPrograms.0"));
    let Value::Number(snapshot_id) = snapshot_id else {
        panic!("snapshot has a numeric ID");
    };
    let diagnostics = call(
        &session,
        "getSemanticDiagnostics",
        &format!(
            "{{\"snapshot\":{snapshot_id},\"project\":{},\"files\":[{}]}}",
            quote(project),
            quote(&file)
        ),
    );
    let Value::Array(diagnostics) = diagnostics else {
        panic!("diagnostics are an array");
    };
    assert_eq!(diagnostics.len(), 5);
    let line_starts = tsrs_core::compute_ecma_line_starts(source);
    for (line, (diagnostic, name)) in
        diagnostics.iter().zip(["alpha", "beta", "gamma", "delta", "epsilon"]).enumerate()
    {
        let byte_start = source.find(name).unwrap();
        let expected_start = source[..byte_start].encode_utf16().count() as f64;
        let expected_column =
            source[line_starts[line] as usize..byte_start].encode_utf16().count() as f64;
        assert_eq!(get(diagnostic, "code"), &Value::Number(7006.0));
        assert_eq!(get(diagnostic, "pos"), &Value::Number(expected_start));
        assert_eq!(get(diagnostic, "end"), &Value::Number(expected_start + name.len() as f64));
        assert_eq!(get(diagnostic, "startPosition.line"), &Value::Number(line as f64));
        assert_eq!(get(diagnostic, "startPosition.character"), &Value::Number(expected_column));
        assert_eq!(
            get(diagnostic, "endPosition.character"),
            &Value::Number(expected_column + name.len() as f64)
        );
    }
    assert!(
        call_err(
            &session,
            "getSemanticDiagnostics",
            r#"{"snapshot":999999,"project":"missing","files":[]}"#
        )
        .contains("snapshot")
    );
}
