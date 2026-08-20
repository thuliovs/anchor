#!/usr/bin/env node

const { spawnSync } = require('node:child_process');

const args = process.argv.slice(2);
const normalizedArgs = args[0] === '--' ? args.slice(1) : args;
const result = spawnSync('jest', normalizedArgs, {
  stdio: 'inherit',
  shell: true,
});

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
