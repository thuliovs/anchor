import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

import Ajv2020 from 'ajv/dist/2020.js';
import type { AnySchema } from 'ajv';

import { MAX_DATAGRAM_BYTES, PROTOCOL_VERSION } from '../index.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const packageRoot = path.resolve(__dirname, '..');
const fixturesDir = path.join(packageRoot, 'fixtures');
const schemaPath = path.join(packageRoot, 'schema', 'motion-sample.v1.schema.json');

function readJson(relativePath: string): unknown {
  return JSON.parse(readFileSync(path.join(packageRoot, relativePath), 'utf8'));
}

test('exports protocol constants', () => {
  assert.equal(PROTOCOL_VERSION, 1);
  assert.equal(MAX_DATAGRAM_BYTES, 1024);
});

test('accepts valid shared fixtures', () => {
  const ajv = new Ajv2020({ allErrors: true, strict: true });
  const schema = readJson('schema/motion-sample.v1.schema.json') as AnySchema;
  const validate = ajv.compile(schema);
  const validFixture = readJson('fixtures/valid/motion-sample.json');

  const isValid = validate(validFixture);

  assert.equal(isValid, true, JSON.stringify(validate.errors, null, 2));
});

test('rejects invalid shared fixtures', () => {
  const ajv = new Ajv2020({ allErrors: true, strict: true });
  const schema = JSON.parse(readFileSync(schemaPath, 'utf8')) as AnySchema;
  const validate = ajv.compile(schema);
  const invalidFixtures = [
    'invalid/unsupported-version.json',
    'invalid/missing-field.json',
    'invalid/out-of-range.json',
    'invalid/unknown-property.json',
  ];

  for (const fixturePath of invalidFixtures) {
    const fixture = JSON.parse(readFileSync(path.join(fixturesDir, fixturePath), 'utf8'));
    const isValid = validate(fixture);
    assert.equal(isValid, false, `${fixturePath} should be invalid`);
  }
});
