#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');

const platforms = Object.freeze({
  'linux-x64-glibc': ['twigsmith-linux-x64-gnu', 'x86_64-unknown-linux-gnu', 'twigsmith'],
  'win32-x64': ['twigsmith-win32-x64', 'x86_64-pc-windows-msvc', 'twigsmith.exe'],
  'darwin-arm64': ['twigsmith-darwin-arm64', 'aarch64-apple-darwin', 'twigsmith'],
  'darwin-x64': ['twigsmith-darwin-x64', 'x86_64-apple-darwin', 'twigsmith'],
});

function selectPlatform(os, arch, report) {
  let key = `${os}-${arch}`;
  if (os === 'linux') {
    if (!report || !report.header || !report.header.glibcVersionRuntime) {
      throw new Error(`unsupported platform ${key}: the Linux package requires glibc`);
    }
    key += '-glibc';
  }
  if (!Object.hasOwn(platforms, key)) throw new Error(`unsupported platform ${key}`);
  return platforms[key];
}

function resolvePayload(metadata, os = process.platform, arch = process.arch) {
  const report = os === 'linux' ? process.report.getReport() : null;
  const [name, target, filename] = selectPlatform(os, arch, report);
  const version = metadata.optionalDependencies && metadata.optionalDependencies[name];
  if (version !== metadata.version) throw new Error('invalid launcher dependency metadata');
  let location;
  try {
    location = require.resolve(`${name}/package.json`);
  } catch (error) {
    throw new Error(`missing ${name}@${version}; reinstall twigsmith@${version} with optional dependencies enabled (do not use --omit=optional)`);
  }
  const dependency = JSON.parse(fs.readFileSync(location, 'utf8'));
  const expectedLibc = os === 'linux' ? ['glibc'] : undefined;
  if (dependency.name !== name || dependency.version !== version ||
      JSON.stringify(dependency.os) !== JSON.stringify([os]) ||
      JSON.stringify(dependency.cpu) !== JSON.stringify([arch]) ||
      JSON.stringify(dependency.libc) !== JSON.stringify(expectedLibc) ||
      !dependency.twigsmithNative || dependency.twigsmithNative.target !== target ||
      dependency.twigsmithNative.file !== filename) {
    throw new Error(`invalid ${name}: expected version ${version} and payload for ${os}/${arch}; reinstall twigsmith@${version}`);
  }
  const executable = path.join(path.dirname(location), filename);
  if (!fs.lstatSync(executable).isFile()) throw new Error(`invalid native executable in ${name}`);
  return executable;
}

function main() {
  try {
    const metadata = JSON.parse(fs.readFileSync(path.join(__dirname, 'package.json'), 'utf8'));
    const executable = resolvePayload(metadata);
    const args = process.argv.slice(2);
    if (process.platform !== 'win32') {
      // Replacing this process preserves the terminal, PID, arguments, signal
      // handling and exit status, including native interactive cancellation.
      if (typeof process.execve !== 'function') {
        throw new Error('this launcher requires a maintained Node.js LTS release with process.execve (Node 22.15 or newer)');
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

module.exports = { selectPlatform, resolvePayload };
if (require.main === module) main();
