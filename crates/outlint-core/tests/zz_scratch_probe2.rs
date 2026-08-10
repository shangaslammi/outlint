use marked_yaml::LoaderOptions;

fn probe_marked(name: &str, body: &str) {
    for dup in [true, false] {
        let opts = LoaderOptions::default()
            .error_on_duplicate_keys(dup)
            .prevent_coercion(true);
        let r = marked_yaml::parse_yaml_with_options(0, body, opts);
        println!(
            "{name} error_on_duplicate_keys={dup}: {}",
            match &r {
                Ok(_) => "Ok".to_string(),
                Err(e) => format!("Err({e})"),
            }
        );
    }
}

#[test]
fn probe2() {
    probe_marked("dup", "a: 1\na: hello\n");
    probe_marked("bigint_then_dup", "big: 99999999999999999999999\na: 1\na: 2\n");
    probe_marked("nested_dup", "outer:\n  a: 1\n  a: 2\n");
}
