mod runner;

use runner::run_fixture_file;

macro_rules! fixture_test {
    ($name:ident, $path:expr) => {
        #[tokio::test]
        async fn $name() {
            let failures: Vec<String> = run_fixture_file($path).await;
            if !failures.is_empty() {
                panic!(
                    "\n{} conformance failure(s):\n  - {}\n",
                    failures.len(),
                    failures.join("\n  - ")
                );
            }
        }
    };
}

fixture_test!(
    conformance_edit_tool,
    "tests/conformance/fixtures/edit_tool.json"
);
fixture_test!(
    conformance_shell_tool,
    "tests/conformance/fixtures/shell_tool.json"
);
fixture_test!(
    conformance_read_tool,
    "tests/conformance/fixtures/read_tool.json"
);
fixture_test!(
    conformance_grep_tool,
    "tests/conformance/fixtures/grep_tool.json"
);
fixture_test!(
    conformance_find_tool,
    "tests/conformance/fixtures/find_tool.json"
);
