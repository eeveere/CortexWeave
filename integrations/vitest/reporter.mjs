// Vitest 4.1.10 reporter used by capture_vitest.mjs. It only serializes
// framework observations; it never decides CortexWeave eligibility.
import { writeFileSync } from 'node:fs';

function suitesFor(test) {
  const suites = [];
  let parent = test.parent;
  while (parent && parent.type === 'suite') {
    suites.unshift(parent.name);
    parent = parent.parent;
  }
  return suites;
}

function errorFacts(errors = []) {
  return errors.map((error) => ({ name: error?.name ?? 'Error', message: error?.message ?? null }));
}

function testFact(test) {
  const result = test.result();
  return {
    module: test.module.relativeModuleId,
    suites: suitesFor(test),
    name: test.name,
    mode: test.mode,
    options: test.options ?? {},
    state: result?.state ?? 'unknown',
    retryCount: result?.retryCount ?? 0,
    repeatCount: result?.repeatCount ?? 0,
    errors: errorFacts(result?.errors),
  };
}

export default class CortexWeaveVitestReporter {
  onInit(context) { this.context = context; }
  onTestRunEnd(modules, unhandledErrors, reason) {
    const config = this.context.config;
    const report = {
      version: this.context.version,
      reason: reason ?? null,
      config: {
        watch: Boolean(config.watch),
        testNamePattern: config.testNamePattern?.source ?? null,
        shard: config.shard ?? null,
        bail: config.bail ?? 0,
        passWithNoTests: Boolean(config.passWithNoTests),
        updateSnapshot: config.snapshotOptions?.updateSnapshot ?? null,
        cache: config.cache !== false,
      },
      unhandledErrors: errorFacts(unhandledErrors),
      modules: modules.map((module) => ({
        relativeModuleId: module.relativeModuleId,
        state: module.state(),
        errors: errorFacts(module.errors()),
        tests: [...module.children.allTests()].map(testFact),
      })),
    };
    writeFileSync(process.env.CORTEXWEAVE_VITEST_RAW, JSON.stringify(report, null, 2));
  }
}
