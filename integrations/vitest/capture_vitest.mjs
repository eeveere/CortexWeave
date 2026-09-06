#!/usr/bin/env node
// Capture one complete Vitest file using the project's installed Vitest.
// No dependency installation, source edit, or CortexWeave submission occurs here.
import { createRequire } from 'node:module';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, relative, resolve, extname, basename } from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const sha = (value) => createHash('blake2s256').update(value).digest('hex');
const fileDigest = (path) => sha(readFileSync(path));
const component = (id, version) => ({ id, version });
const empty = () => ({ discovered: 0, executed: 0, passed: 0, assertion_failed: 0, errored: 0, skipped: 0, todo: 0, pending: 0, expected_failed: 0, unexpected_successful: 0 });

function args() {
  const values = process.argv.slice(2); const take = (name) => { const i = values.indexOf(name); return i >= 0 ? values[i + 1] : null; };
  const workspace = take('--workspace'), testFile = take('--test-file'), output = take('--output');
  if (!workspace || !testFile || !output) throw new Error('usage: capture_vitest.mjs --workspace <path> --test-file <relative-path> --output <bundle.json> [--run-id <id>]');
  return { workspace: resolve(workspace), testFile, output: resolve(output), runId: take('--run-id') ?? randomUUID() };
}
function status(test) {
  if (test.mode === 'todo') return 'todo';
  if (test.mode === 'skip' || test.state === 'skip') return 'skipped';
  if (test.state === 'pass' || test.state === 'passed') return 'passed';
  if (test.state === 'fail' || test.state === 'failed') return test.errors?.[0]?.name === 'AssertionError' ? 'assertion_failed' : 'errored';
  return 'pending';
}
function language(path) { return ['.ts', '.tsx'].includes(extname(path)) ? 'typescript' : 'javascript'; }

const input = args(); const selected = resolve(input.workspace, input.testFile);
if (!selected.startsWith(`${input.workspace}\\`) && selected !== input.workspace) throw new Error('test file must be inside --workspace');
if (!existsSync(selected)) throw new Error(`test file does not exist: ${input.testFile}`);
mkdirSync(dirname(input.output), { recursive: true });
const rawPath = input.output.replace(/\.json$/i, '.raw.json');
if (existsSync(input.output) || existsSync(rawPath)) {
  throw new Error(`refusing to overwrite existing capture artifact: ${input.output}`);
}
const requireFromWorkspace = createRequire(resolve(input.workspace, 'package.json'));
const packagePath = requireFromWorkspace.resolve('vitest/package.json');
const vitestPackage = JSON.parse(readFileSync(packagePath, 'utf8'));
const binPath = resolve(dirname(packagePath), vitestPackage.bin.vitest);
const before = fileDigest(selected);
const reporterPath = resolve(dirname(fileURLToPath(import.meta.url)), 'reporter.mjs');
const child = spawnSync(process.execPath, [binPath, 'run', input.testFile, '--reporter', reporterPath, '--update=none'], {
  cwd: input.workspace, encoding: 'utf8', env: { ...process.env, CORTEXWEAVE_VITEST_RAW: rawPath },
});
const after = fileDigest(selected); const raw = existsSync(rawPath) ? JSON.parse(readFileSync(rawPath, 'utf8')) : null;
const cases = (raw?.modules ?? []).flatMap((module) => module.tests.map((test) => ({
  identity: { namespace: test.suites, name: test.name }, status: status(test), error_class: test.errors?.[0]?.name ?? null,
  retry_configured: Number(test.options?.retry ?? 0) > 0, retry_count: Number(test.retryCount ?? 0), repeat_configured: Number(test.options?.repeats ?? 0) > 0, repeat_count: Number(test.repeatCount ?? 0), flaky: Number(test.retryCount ?? 0) > 0,
})));
const parents = empty(); parents.discovered = cases.length;
for (const test of cases) { if (!['skipped', 'todo', 'pending'].includes(test.status)) parents.executed++; parents[test.status]++; }
const rawBytes = Buffer.from(JSON.stringify(raw ?? { stderr: child.stderr, stdout: child.stdout })); writeFileSync(rawPath, rawBytes);
const settings = raw?.config ?? {}; const errors = [ ...(raw?.unhandledErrors ?? []).map((error) => ({ phase: 'runtime', error_class: error.name, message: error.message })), ...(raw?.modules ?? []).flatMap((module) => module.errors.map((error) => ({ phase: 'runtime', error_class: error.name, message: error.message }))) ];
const completed = child.status !== null && raw !== null;
const bundle = { contract: 'cortexweave.test_run_result', version: 1, producer: component('cortexweave.vitest_capture', '1'), runner: component('vitest', vitestPackage.version), runtime: component('node', process.versions.node), profile: component('cortexweave.vitest.file', '1'), run_id: input.runId, operation: 'test', completed, exit_code: child.status ?? 1, language: language(selected), environment_fingerprint: sha(JSON.stringify({ node: process.versions.node, vitest: vitestPackage.version, executable: process.execPath })), selection: { project_root: '.', import_root: '.', test_file: relative(input.workspace, selected).replaceAll('\\', '/'), kind: 'file', value: relative(input.workspace, selected).replaceAll('\\', '/'), settings_digest: sha(JSON.stringify(settings)), inventory_complete: completed }, cases, children: [], counts: { parents, children: { observed: 0, passed: 0, assertion_failed: 0, errored: 0, skipped: 0, todo: 0, pending: 0, expected_failed: 0, unexpected_successful: 0 } }, errors, settings: { focused_only: cases.some((_, i) => (raw?.modules ?? []).flatMap((m) => m.tests)[i]?.mode === 'only'), name_filter: settings.testNamePattern !== null && settings.testNamePattern !== undefined, sharded: settings.shard !== null && settings.shard !== undefined, bail: Boolean(settings.bail), watch: Boolean(settings.watch), snapshot_mode: 'disabled', pass_with_no_tests: Boolean(settings.passWithNoTests), execution_origin: 'current' }, verification_inputs: [{ kind: 'test_file', path: relative(input.workspace, selected).replaceAll('\\', '/'), observed: { state: 'present', before_digest: before, after_digest: after } }], artifacts: [{ kind: 'vitest_reporter', digest: sha(rawBytes), reference: basename(rawPath) }] };
writeFileSync(input.output, JSON.stringify(bundle, null, 2) + '\n'); process.exit(child.status ?? 1);
