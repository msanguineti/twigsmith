#!/usr/bin/env node
'use strict';
// No environment variable can relax the release origin policy. Loopback test
// transport is bound to purpose-built fixture metadata by the test packer only.
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const https = require('node:https');
const http = require('node:http');
const zlib = require('node:zlib');
const RELEASE_AGENT = new https.Agent({proxyEnv: process.env, rejectUnauthorized: true});
const MAX_ARCHIVE = 128 * 1024 * 1024;
const MAX_EXPANDED = 512 * 1024 * 1024;
const TARGETS = Object.freeze({
  'linux-x64-glibc': 'x86_64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
});
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');

function assertRuntime() {
  const [major, minor] = process.versions.node.split('.').map(Number);
  if (!((major === 22 && minor >= 21) || (major === 24 && minor >= 5) || major > 24)) {
    throw new Error('installer requires Node 22.21+ in the 22.x line, or Node 24.5+');
  }
}

function detectTarget(options = {}, host = {os: process.platform, cpu: process.arch,
  glibc: process.platform === 'linux' ? process.report.getReport().header.glibcVersionRuntime : null}) {
  if (options.target) {
    if (options.os || options.cpu || options.libc) throw new Error('--target cannot be combined with --os/--cpu/--libc');
    if (!Object.values(TARGETS).includes(options.target)) throw new Error('unsupported target');
    return options.target;
  }
  const os = options.os || host.os, cpu = options.cpu || host.cpu;
  if (options.libc && os !== 'linux') throw new Error('--libc is only valid for Linux');
  let key = `${os}-${cpu}`;
  if (os === 'linux') {
    if ((options.libc || 'glibc') !== 'glibc') throw new Error('Linux requires glibc 2.35 or newer');
    // Explicit foreign selection verifies/downloads bytes only. Native automatic
    // selection and explicit selection on Linux retain the supported ABI floor.
    if (host.os === 'linux') {
      const match = /^(\d+)\.(\d+)(?:\.|$)/.exec(host.glibc || '');
      if (!match || Number(match[1]) < 2 || (Number(match[1]) === 2 && Number(match[2]) < 35))
        throw new Error('Linux requires glibc 2.35 or newer');
    }
    key += '-glibc';
  }
  if (!Object.hasOwn(TARGETS, key)) throw new Error(`unsupported platform ${os}/${cpu}`);
  return TARGETS[key];
}
function parseOptions(args, env = process.env) {
  const options = {};
  for (let i = 0; i < args.length; i++) {
    const match = /^--(target|os|cpu|libc)(?:=(.+))?$/.exec(args[i]);
    if (!match) throw new Error(`unknown installer option: ${args[i]}`);
    if (Object.hasOwn(options, match[1])) throw new Error(`duplicate installer option: --${match[1]}`);
    const value = match[2] || args[++i];
    if (!value || value.startsWith('-')) throw new Error(`missing value for --${match[1]}`);
    options[match[1]] = value;
  }
  // CLI wins over npm's install flags; target alone overrides the whole tuple.
  if (!options.target) for (const key of ['os', 'cpu', 'libc']) {
    if (!options[key] && env[`npm_config_${key}`]) options[key] = env[`npm_config_${key}`];
  }
  return options;
}
function validateMetadata(metadata, packageVersion) {
  if (metadata.schema !== 1 || metadata.version !== packageVersion || !metadata.targets ||
      !Object.keys(metadata.targets).length) throw new Error('invalid native metadata version');
  if (!metadata.fixture && Object.keys(metadata.targets).sort().join() !== Object.values(TARGETS).sort().join())
    throw new Error('native metadata must contain all four supported targets');
  for (const [target, entry] of Object.entries(metadata.targets)) {
    if (!Object.values(TARGETS).includes(target) || !/^[0-9a-f]{64}$/.test(entry.archive_sha256) ||
        !/^[0-9a-f]{64}$/.test(entry.payload_sha256) ||
        entry.archive !== `twigsmith-${metadata.version}-${target}${target === TARGETS['win32-x64'] ? '.zip' : '.tar.gz'}` ||
        entry.payload !== (target === TARGETS['win32-x64'] ? 'twigsmith.exe' : 'twigsmith'))
      throw new Error('invalid native target metadata');
    const stem = entry.archive.replace(/\.(tar\.gz|zip)$/, '');
    const names = target === TARGETS['win32-x64'] ? ['twigsmith.exe','tgi.exe','LICENSE','README.txt'] : ['twigsmith','tgi','LICENSE','README.txt'];
    if (!entry.members || Object.keys(entry.members).sort().join() !== names.map(n => `${stem}/${n}`).sort().join())
      throw new Error('invalid native archive member metadata');
    if (Object.values(entry.members).reduce((n, member) => n + member.size, 0) > MAX_EXPANDED) throw new Error('native members exceed size limit');
    for (const member of Object.values(entry.members)) if (!Number.isSafeInteger(member.size) ||
        member.size < 0 || member.size > MAX_EXPANDED || !/^[0-9a-f]{64}$/.test(member.sha256) ||
        ![0o644,0o755].includes(member.mode)) throw new Error('invalid native member identity');
    const payload = entry.members[`${stem}/${entry.payload}`];
    if (payload.sha256 !== entry.payload_sha256) throw new Error('payload hash metadata disagrees');
    const url = new URL(entry.url);
    if (metadata.fixture) {
      if (metadata.fixture !== 'loopback-test-only' || url.origin !== metadata.fixture_origin ||
          url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || !url.port)
        throw new Error('invalid fixture origin');
    } else if (entry.url !== `https://github.com/msanguineti/twigsmith/releases/download/v${metadata.version}/${entry.archive}`)
      throw new Error('native URL must identify the exact GitHub release asset');
    if (url.username || url.password || url.search || url.hash) throw new Error('invalid release asset URL');
  }
}
function allowedURL(url, metadata, initial = false, entry) {
  if (url.username || url.password || url.hash) throw new Error('download URL contains credentials or fragment');
  if (metadata.fixture) {
    if (url.origin !== metadata.fixture_origin || url.protocol !== 'http:') throw new Error('fixture redirect left loopback origin');
  } else if (url.protocol !== 'https:' || (url.port && url.port !== '443') ||
      !['github.com','release-assets.githubusercontent.com','objects.githubusercontent.com'].includes(url.hostname))
    throw new Error('download redirect has an unapproved origin');
  if (initial && url.href !== entry.url) throw new Error('download URL changed');
  if (!metadata.fixture && url.hostname === 'github.com' && url.href !== entry.url)
    throw new Error('GitHub redirect changed release identity');
}
async function download(entry, metadata) {
  assertRuntime();
  const deadline = Date.now() + 60000;
  async function get(url, redirects) {
    allowedURL(url, metadata, redirects === 0, entry);
    if (redirects > 5 || Date.now() >= deadline) throw new Error('download redirect/time limit exceeded');
    return new Promise((resolve, reject) => {
      const request = (url.protocol === 'https:' ? https : http).get(url, {
        agent: url.protocol === 'https:' ? RELEASE_AGENT : undefined,
        headers: {'User-Agent': 'twigsmith-npm-installer', 'Accept': 'application/octet-stream'},
        timeout: Math.min(15000, deadline - Date.now()),
      }, response => {
        const fail = message => { response.destroy(); reject(new Error(message)); };
        if ([301,302,303,307,308].includes(response.statusCode)) {
          response.resume();
          if (!response.headers.location) return fail('redirect omitted location');
          let next; try { next = new URL(response.headers.location, url); } catch { return fail('invalid redirect'); }
          get(next, redirects + 1).then(resolve, reject); return;
        }
        if (response.statusCode !== 200) return fail(`native download returned HTTP ${response.statusCode}`);
        const length = response.headers['content-length'];
        if (length && (!/^\d+$/.test(length) || Number(length) > MAX_ARCHIVE)) return fail('native archive exceeds size limit');
        const parts = []; let total = 0;
        response.on('data', chunk => {
          total += chunk.length;
          if (total > MAX_ARCHIVE) return fail('native archive exceeds size limit');
          parts.push(chunk);
        });
        response.on('error', () => reject(new Error('native download interrupted')));
        response.on('aborted', () => reject(new Error('native download interrupted')));
        response.on('end', () => {
          if (length && total !== Number(length)) return reject(new Error('native download truncated'));
          resolve(Buffer.concat(parts));
        });
      });
      const timedOut = () => { reject(new Error('native download timed out')); request.destroy(); };
      const timer = setTimeout(timedOut, Math.max(1, deadline - Date.now()));
      request.on('close', () => clearTimeout(timer));
      request.on('timeout', timedOut);
      request.on('error', () => reject(new Error('native download failed; check network access and Node CA trust')));
    });
  }
  return get(new URL(entry.url), 0);
}
function checkedMember(name, data, mode, entry, seen) {
  if (!Object.hasOwn(entry.members, name) || seen.has(name) || name.includes('..') || name.includes('\\'))
    throw new Error('unexpected, unsafe or duplicate native archive member');
  const expected = entry.members[name];
  if (data.length !== expected.size || mode !== expected.mode || hash(data) !== expected.sha256)
    throw new Error('native archive member identity differs');
  seen.set(name, data);
}
function tarMembers(archive, entry) {
  const bytes = zlib.gunzipSync(archive, {maxOutputLength: MAX_EXPANDED});
  const seen = new Map();
  let offset = 0, ended = false;
  const string = b => {
    const n = b.indexOf(0);
    return b.subarray(0, n < 0 ? b.length : n).toString('utf8');
  };
  const octal = b => {
    const text = string(b).trim();
    if (!/^[0-7]+$/.test(text)) throw new Error('invalid tar numeric field');
    return parseInt(text, 8);
  };
  while (offset + 512 <= bytes.length) {
    const header = bytes.subarray(offset, offset + 512);
    offset += 512;
    if (header.every(b => b === 0)) {
      if (bytes.length - offset < 512) throw new Error('missing tar trailer');
      ended = true;
      if (!bytes.subarray(offset).every(b => b === 0)) throw new Error('nonzero tar trailer');
      break;
    }
    let checksum = 0;
    for (let i = 0; i < 512; i++) checksum += i >= 148 && i < 156 ? 32 : header[i];
    if (checksum !== octal(header.subarray(148,156)) || string(header.subarray(257,263)) !== 'ustar' ||
        ![0,48].includes(header[156]) || string(header.subarray(157,257))) {
      throw new Error('invalid or nonregular tar member');
    }
    const name = string(header.subarray(0,100)), prefix = string(header.subarray(345,500));
    const size = octal(header.subarray(124,136));
    if (size > MAX_EXPANDED || offset + size > bytes.length) throw new Error('truncated tar member');
    checkedMember(prefix ? `${prefix}/${name}` : name, bytes.subarray(offset,offset+size), octal(header.subarray(100,108)), entry, seen);
    offset += Math.ceil(size/512)*512;
  }
  if (!ended || offset > bytes.length) throw new Error('missing tar trailer');
  return seen;
}
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function zipMembers(bytes, entry) {
  // Our native packer emits ordinary non-ZIP64 ZIPs with no comments/descriptors.
  if (bytes.length < 22) throw new Error('truncated ZIP');
  const end = bytes.length-22;
  if (bytes.readUInt32LE(end)!==0x06054b50 || bytes.readUInt16LE(end+4) || bytes.readUInt16LE(end+6) ||
      bytes.readUInt16LE(end+20) || bytes.readUInt16LE(end+8)!==bytes.readUInt16LE(end+10)) {
    throw new Error('invalid ZIP directory');
  }
  const count=bytes.readUInt16LE(end+10), directorySize=bytes.readUInt32LE(end+12), start=bytes.readUInt32LE(end+16);
  if (start+directorySize!==end || count!==Object.keys(entry.members).length) {
    throw new Error('invalid ZIP members');
  }
  const seen = new Map();
  let pos = start, local = 0;
  for (let i = 0; i < count; i++) {
    if(pos+46>end || bytes.readUInt32LE(pos)!==0x02014b50) {
      throw new Error('invalid ZIP member');
    }
    const flags=bytes.readUInt16LE(pos+8), method=bytes.readUInt16LE(pos+10), crc=bytes.readUInt32LE(pos+16);
    const size=bytes.readUInt32LE(pos+24), compressed=bytes.readUInt32LE(pos+20), nameLength=bytes.readUInt16LE(pos+28);
    const extra=bytes.readUInt16LE(pos+30), comment=bytes.readUInt16LE(pos+32), attrs=bytes.readUInt32LE(pos+38);
    const location=bytes.readUInt32LE(pos+42), name=bytes.subarray(pos+46,pos+46+nameLength).toString('utf8');
    if(flags!==0 || ![0,8].includes(method) || extra || comment || pos+46+nameLength>end || location!==local ||
       size>MAX_EXPANDED || ((attrs>>>16)&0xf000)!==0x8000 || bytes.readUInt16LE(pos+34)) {
      throw new Error('unsupported ZIP member type/flags');
    }
    if(location+30>start || bytes.readUInt32LE(location)!==0x04034b50 || bytes.readUInt16LE(location+6)!==flags ||
       bytes.readUInt16LE(location+8)!==method || bytes.readUInt32LE(location+14)!==crc ||
       bytes.readUInt32LE(location+18)!==compressed || bytes.readUInt32LE(location+22)!==size ||
       bytes.readUInt16LE(location+26)!==nameLength || bytes.readUInt16LE(location+28) ||
       !bytes.subarray(location+30,location+30+nameLength).equals(bytes.subarray(pos+46,pos+46+nameLength))) {
      throw new Error('ZIP local directory disagrees');
    }
    const dataStart=location+30+nameLength, dataEnd=dataStart+compressed;
    if(dataEnd>start) throw new Error('truncated ZIP member');
    if (!Object.hasOwn(entry.members,name) || entry.members[name].size !== size) {
      throw new Error('unexpected ZIP member size');
    }
    const data=method===0 ? bytes.subarray(dataStart,dataEnd) : zlib.inflateRawSync(bytes.subarray(dataStart,dataEnd), {maxOutputLength:Math.max(1,size)});
    if(data.length!==size || crc32(data)!==crc) {
      throw new Error('ZIP size or CRC differs');
    }
    checkedMember(name,data,(attrs>>>16)&0o7777,entry,seen);
    local = dataEnd;
    pos += 46 + nameLength;
  }
  if(pos!==end || local!==start) {
    throw new Error('unexpected ZIP bytes');
  }
  return seen;
}
function unpack(archive, entry) {
  if (hash(archive)!==entry.archive_sha256) throw new Error('native archive SHA-256 differs from the package metadata');
  const members=entry.archive.endsWith('.zip') ? zipMembers(archive,entry) : tarMembers(archive,entry);
  if(members.size!==Object.keys(entry.members).length) {
    throw new Error('missing native archive member');
  }
  const payload=members.get(`${entry.archive.replace(/\.(tar\.gz|zip)$/,'')}/${entry.payload}`);
  if(!payload || hash(payload)!==entry.payload_sha256) {
    throw new Error('native executable SHA-256 differs');
  }
  return payload;
}
function regularFile(file, limit=MAX_EXPANDED) {
  const stat=fs.lstatSync(file);
  if(!stat.isFile() || stat.isSymbolicLink() || stat.size > limit) {
    throw new Error('native installation file is not regular or exceeds its size limit');
  }
  return fs.readFileSync(file);
}
function nativeRoot(directory) {
  const root=path.join(directory,'.native');
  try {
    fs.mkdirSync(root, {mode:0o755});
  } catch (error) {
    if (error.code !== 'EEXIST') throw error;
  }
  const stat = fs.lstatSync(root);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('invalid native installation directory');
  return root;
}
function cleanAbandoned(root) {
  for (const name of fs.readdirSync(root)) {
    const match=/^\.(?:install|state)-([1-9][0-9]*)-/.exec(name);
    if (!match || !Number.isSafeInteger(Number(match[1]))) continue;
    try {
      process.kill(Number(match[1]), 0);
    } catch (error) {
      // EPERM is an alive process we cannot inspect. PID reuse conservatively
      // retains old staging data rather than disturbing another installer.
      if(error.code==='ESRCH') fs.rmSync(path.join(root,name),{recursive:true,force:true});
    }
  }
}
function readMetadata(directory) {
  assertRuntime();
  const metadata=JSON.parse(regularFile(path.join(directory,'native.json'),128*1024).toString('utf8'));
  const pkg=JSON.parse(fs.readFileSync(path.join(directory,'package.json'),'utf8'));
  validateMetadata(metadata, pkg.version);
  return metadata;
}
function installedPayload(directory, metadata) {
  const state=JSON.parse(regularFile(path.join(directory,'.native','installed.json'),4096).toString('utf8'));
  if (Object.keys(state).sort().join() !== 'directory,target') throw new Error('invalid installed state');
  const entry=metadata.targets[state.target];
  if(!entry || state.directory!==`${state.target}-${entry.payload_sha256}`) {
    throw new Error('invalid installed native identity');
  }
  const root=nativeRoot(directory), subdir=path.join(root,state.directory);
  if(!fs.lstatSync(subdir).isDirectory() || fs.lstatSync(subdir).isSymbolicLink()) {
    throw new Error('invalid native payload directory');
  }
  const executable=path.join(subdir,entry.payload);
  if(hash(regularFile(executable))!==entry.payload_sha256) {
    throw new Error('installed native executable SHA-256 differs');
  }
  return {target:state.target,executable};
}
async function install(directory=__dirname, options={}) {
  const metadata=readMetadata(directory), target=detectTarget(options), entry=metadata.targets[target];
  if(!entry) throw new Error('selected target is absent from this fixture');
  const root=nativeRoot(directory), destination=`${target}-${entry.payload_sha256}`;
  cleanAbandoned(root);
  const final=path.join(root,destination), temporary=fs.mkdtempSync(path.join(root,`.install-${process.pid}-`));
  let stateTemp;
  try {
    const payload=unpack(await download(entry,metadata),entry);
    const executable=path.join(temporary,entry.payload);
    fs.writeFileSync(executable,payload,{flag:'wx',mode:0o755});
    // Never remove/overwrite a previously ready directory, including on Windows.
    try {
      fs.renameSync(temporary, final);
    } catch (error) {
      if(!['EEXIST','ENOTEMPTY','EPERM'].includes(error.code)) throw error;
      const stat=fs.lstatSync(final);
      if(!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('conflicting native installation directory');
      let valid=false;
      try {
        valid = hash(regularFile(path.join(final, entry.payload))) === entry.payload_sha256;
      } catch { /* explicit repair below */ }
      if(!valid) {
        // Move a damaged directory aside first. Never overwrite a running Windows
        // executable. A failed replacement leaves a retryable diagnostic, and a
        // concurrently completed valid directory is revalidated by the next run.
        const damaged=path.join(root,`.damaged-${crypto.randomBytes(12).toString('hex')}`);
        fs.renameSync(final,damaged);
        try {
          fs.renameSync(temporary, final);
        } catch (replacementError) {
          try { fs.renameSync(damaged,final); } catch { /* retain damaged bytes for explicit cleanup */ }
          throw replacementError;
        }
        fs.rmSync(damaged,{recursive:true,force:true});
      }
    }
    stateTemp=path.join(root,`.state-${process.pid}-${crypto.randomBytes(12).toString('hex')}`);
    fs.writeFileSync(stateTemp,JSON.stringify({target,directory:destination})+'\n',{flag:'wx',mode:0o644});
    fs.renameSync(stateTemp,path.join(root,'installed.json'));
    console.error(`twigsmith: installed ${target} (selection verified; execution requires a compatible host)`);
    return target;
  } finally {
    fs.rmSync(temporary, {recursive:true, force:true});
    if (stateTemp) fs.rmSync(stateTemp, {force:true});
  }
}
async function main() {
  try {
    await install(__dirname, parseOptions(process.argv.slice(2)));
  } catch (error) {
    console.error(`twigsmith installer: ${error.message}`);
    process.exitCode = 1;
  }
}
module.exports={detectTarget,parseOptions,validateMetadata,allowedURL,download,unpack,tarMembers,zipMembers,hash,install,readMetadata,installedPayload};
if (require.main === module) main();
