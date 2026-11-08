#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');

const installer = require('./installer.cjs');

function resolvePayload() {
  let installed;
  try {
    const metadata = installer.readMetadata(__dirname);
    installed = installer.installedPayload(__dirname, metadata);
    if (installed.target !== installer.detectTarget()) throw new Error('installed target is incompatible with this host');
  } catch (error) {
    throw new Error(`${error.message}; run twigsmith-install to install or repair the native executable (network access required)`);
  }
  return installed.executable;
}

function main() {
  try {
    const executable = resolvePayload();
    const args = process.argv.slice(2);
    if (process.platform !== 'win32') {
      // Replacing this process preserves the terminal, PID, arguments, signal
      // handling and exit status, including native interactive cancellation.
      if (typeof process.execve !== 'function') {
        throw new Error('this launcher requires process.execve in Node 22.21+ (22.x) or Node 24.5+');
      }
      process.execve(executable, [executable, ...args], process.env);
      return;
    }
    const child = spawn(executable, args, { stdio: 'inherit', shell: false });
    // Windows broadcasts console Ctrl+C to both processes. child.kill('SIGINT')
    // would forcefully terminate the native child instead of allowing recovery.
    const interrupt = () => {};
    process.on('SIGINT', interrupt);
    process.on('SIGBREAK', interrupt);
    child.once('error', error => {
      console.error(`twigsmith: cannot start native executable: ${error.message}`);
      process.exitCode = 1;
    });
    child.once('exit', (code, signal) => {
      process.removeListener('SIGINT', interrupt);
      process.removeListener('SIGBREAK', interrupt);
      process.exitCode = code === null ? (signal === 'SIGINT' ? 130 : 1) : code;
    });
  } catch (error) {
    console.error(`twigsmith: ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = { resolvePayload };
if (require.main === module) main();
