import test from 'node:test';
import assert from 'node:assert/strict';

import { parseCliArgs, parseIntegerOption } from '../src/index.js';

test('parseCliArgs ignores the pnpm argument separator', () => {
  const options = parseCliArgs(['--', '--duration', '5', '--pattern', 'sine']);

  assert.equal(options.durationSeconds, 5);
  assert.equal(options.pattern, 'sine');
  assert.equal(options.host, '127.0.0.1');
  assert.equal(options.port, 57_421);
  assert.equal(options.rate, 60);
});

test('parseCliArgs rejects partially valid rate values', () => {
  assert.throws(
    () => parseCliArgs(['--rate', '60oops']),
    /rate must be an integer between 1 and 120/,
  );
  assert.throws(
    () => parseCliArgs(['--rate', '1.5']),
    /rate must be an integer between 1 and 120/,
  );
  assert.throws(
    () => parseCliArgs(['--rate', 'NaN']),
    /rate must be an integer between 1 and 120/,
  );
  assert.throws(
    () => parseCliArgs(['--rate', 'Infinity']),
    /rate must be an integer between 1 and 120/,
  );
});

test('parseCliArgs rejects partially valid port values', () => {
  assert.throws(
    () => parseCliArgs(['--port', '57421junk']),
    /port must be an integer between 1 and 65535/,
  );
  assert.throws(
    () => parseCliArgs(['--port', '']),
    /port must be an integer between 1 and 65535/,
  );
  assert.throws(
    () => parseCliArgs(['--port', '0']),
    /port must be an integer between 1 and 65535/,
  );
  assert.throws(
    () => parseCliArgs(['--port', '65536']),
    /port must be an integer between 1 and 65535/,
  );
});

test('parseIntegerOption accepts only complete decimal integers', () => {
  assert.equal(parseIntegerOption('60', 'rate', 1, 120), 60);

  for (const invalidValue of ['60oops', '57421junk', '1.5', 'NaN', 'Infinity', '', '-1', '121']) {
    assert.throws(
      () => parseIntegerOption(invalidValue, 'rate', 1, 120),
      /rate must be an integer between 1 and 120/,
    );
  }
});
