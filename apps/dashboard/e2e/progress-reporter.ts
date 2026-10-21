import { writeSync } from 'node:fs';
import type { Reporter, TestCase } from 'vitest/node';

export default class ProgressReporter implements Reporter {
  onTestCaseResult(test: TestCase): void {
    const result = test.result();
    // Synchronous output survives a later outer-workflow termination before
    // onTestRunEnd, preserving the original failure rather than only its name.
    writeSync(1, `${JSON.stringify({
      event: 'browser-test-completed',
      at: new Date().toISOString(),
      file: test.module.moduleId,
      name: test.fullName,
      state: result.state,
      errors: result.errors?.map((error) => ({
        name: error.name,
        message: error.message,
        stack: error.stack,
      })),
    })}\n`);
  }
}
