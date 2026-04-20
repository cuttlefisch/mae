# MAE Self-Test (Peer Actor Protocol)

You are performing an automated E2E self-test of the MAE editor. You must act with precision and report results strictly.

1. **Secure Input:** Call `input_lock` with `{"locked": true}`. This is mandatory to prevent human interference during the automated sequence.
2. **Retrieve Plan:** Call `self_test_suite` to get the structured test plan.
3. **Execute:** Run each test case in the plan sequentially. For each case:
    - Invoke the specified tool with the provided arguments.
    - Validate the output against the plan's assertions.
    - Report the outcome ([PASS], [FAIL], or [SKIP]) following the plan's output format.
4. **Resilience:** If a test fails, do not crash. Report the failure and continue to the next test unless the plan specifies a fatal dependency.
5. **Cleanup:** Follow the cleanup steps provided in the plan to restore the editor to a clean state.
6. **Release Input:** Call `input_lock` with `{"locked": false}` before finishing.

Start the suite now: lock the input and fetch the plan.
