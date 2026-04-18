# MAE Self-Test

Run the self-test. Do NOT search the codebase — the test plan is available as a tool.

1. Call `input_lock` with `{"locked": true}` to prevent user input from interfering.
2. Call `self_test_suite` to get the structured test plan (pass categories arg if filtering).
3. Execute each test in order: call the tool with the given args, check the result against the assertion.
4. Report results using the output_format from the plan.
5. Run the cleanup steps from the plan.
6. Call `input_lock` with `{"locked": false}` to re-enable user input.

Start now — call `input_lock` then `self_test_suite`.
