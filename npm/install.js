#!/usr/bin/env node

const os = require('os');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const NAME = 'rmsg';
const VERSION = '0.1.8';
const REPO = 'hgwk/rmsg';
const BIN_DIR = path.join(os.homedir(), '.rmsg', 'bin');
const BIN_PATH = path.join(BIN_DIR, NAME + (os.platform() === 'win32' ? '.exe' : ''));

function getPlatform() {
  const p = os.platform();
  const a = os.arch();
  if (p === 'darwin') return a === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin';
  if (p === 'linux') return a === 'arm64' ? 'aarch64-unknown-linux-gnu' : 'x86_64-unknown-linux-gnu';
  throw new Error(`Unsupported platform: ${p}/${a}`);
}

function getUrl(platform) {
  return `https://github.com/${REPO}/releases/download/v${VERSION}/${NAME}-v${VERSION}-${platform}.tar.gz`;
}

async function install() {
  const platform = getPlatform();
  const url = getUrl(platform);

  fs.mkdirSync(BIN_DIR, { recursive: true });

  const tarPath = path.join(BIN_DIR, `${NAME}.tar.gz`);
  console.log(`Downloading ${url}...`);
  execSync(`curl -L -o "${tarPath}" "${url}"`, { stdio: 'inherit' });

  console.log('Extracting...');
  const extractDir = path.join(BIN_DIR, 'extract');
  fs.mkdirSync(extractDir, { recursive: true });
  execSync(`tar -xzf "${tarPath}" -C "${extractDir}"`, { stdio: 'inherit' });

  const extracted = fs.readdirSync(extractDir).find(f => f.startsWith(NAME));
  if (extracted) {
    fs.copyFileSync(path.join(extractDir, extracted), BIN_PATH);
  } else {
    fs.copyFileSync(path.join(extractDir, NAME), BIN_PATH);
  }
  fs.chmodSync(BIN_PATH, 0o755);
  fs.rmSync(tarPath, { force: true });
  fs.rmSync(extractDir, { recursive: true, force: true });

  console.log(`Installed ${NAME} v${VERSION} to ${BIN_PATH}`);
}

function run() {
  if (!fs.existsSync(BIN_PATH)) {
    console.error(`${NAME} binary not found. Run 'npm install -g @hgwk/rmsg' first.`);
    process.exit(1);
  }
  const args = process.argv.slice(2).map(a => JSON.stringify(a)).join(' ');
  const { status } = require('child_process').spawnSync(BIN_PATH, process.argv.slice(2), { stdio: 'inherit' });
  process.exit(status || 0);
}

if (process.argv.includes('--install')) {
  install().catch(e => { console.error(e.message); process.exit(1); });
} else {
  run();
}
