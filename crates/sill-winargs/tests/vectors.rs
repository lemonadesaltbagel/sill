//! Checks the encoder against every vector in `spec/argv-test-vectors.json`.
//! This runs on any OS; the conformance suite re-checks the parse side on real Windows.

use serde::Deserialize;
use sill_winargs::{encode_bat, encode_exe, encode_exe_bytes};

#[derive(Deserialize)]
struct Spec {
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Generator {
    index: usize,
    char: String,
    count: usize,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    argv: Vec<String>,
    target_kind: String,
    expected_cmdline: Option<String>,
    expected_error: Option<String>,
    argv_generator: Option<Generator>,
    argv_hex: Option<Vec<Option<String>>>,
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn load() -> Vec<Vector> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/argv-test-vectors.json");
    let spec: Spec = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    spec.vectors
}

fn encode(v: &Vector) -> Result<String, String> {
    let mut argv = v.argv.clone();
    if let Some(g) = &v.argv_generator {
        argv[g.index] = g.char.repeat(g.count);
    }
    if let Some(hex) = &v.argv_hex {
        let bytes: Vec<Vec<u8>> = argv
            .iter()
            .zip(hex)
            .map(|(a, h)| h.as_deref().map(hex_to_bytes).unwrap_or_else(|| a.as_bytes().to_vec()))
            .collect();
        let refs: Vec<&[u8]> = bytes.iter().map(Vec::as_slice).collect();
        return encode_exe_bytes(&refs).map_err(|e| e.to_string());
    }
    match v.target_kind.as_str() {
        "exe" => encode_exe(&argv),
        "bat" => encode_bat(&argv),
        other => panic!("unknown target_kind {other}"),
    }
    .map_err(|e| e.to_string())
}

#[test]
fn all_spec_vectors() {
    let vectors = load();
    assert_eq!(vectors.len(), 59, "vector count changed; update this test deliberately");
    let mut failures = Vec::new();
    for v in &vectors {
        let got = encode(v);
        let want = match (&v.expected_cmdline, &v.expected_error) {
            (Some(c), None) => Ok(c.clone()),
            (None, Some(e)) => Err(e.clone()),
            _ => panic!("{}: needs exactly one of expected_cmdline/expected_error", v.name),
        };
        if got != want {
            failures.push(format!("{}:\n  want {want:?}\n  got  {got:?}", v.name));
        }
    }
    assert!(failures.is_empty(), "{} vector(s) failed:\n{}", failures.len(), failures.join("\n"));
}
