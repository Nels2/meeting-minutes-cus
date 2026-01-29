#!/usr/bin/env node
/**
 * Build and stage the llama-helper sidecar into src-tauri/binaries.
 *
 * Usage:
 *   node scripts/build-sidecar.js --release
 *   node scripts/build-sidecar.js --debug
 *   node scripts/build-sidecar.js --release --detect-gpu
 *
 * Features:
 * - Uses TAURI_GPU_FEATURE if provided.
 * - Optional GPU auto-detect with --detect-gpu.
 */

const { execSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const args = process.argv.slice(2);
const isRelease = args.includes('--release');
const shouldDetectGpu = args.includes('--detect-gpu');

const repoRoot = path.resolve(__dirname, '..', '..');
const tauriDir = path.join(repoRoot, 'frontend', 'src-tauri');
const binariesDir = path.join(tauriDir, 'binaries');
const targetDir = path.join(repoRoot, 'target', isRelease ? 'release' : 'debug');
const helperDir = path.join(repoRoot, 'llama-helper');

if (!fs.existsSync(helperDir)) {
  console.error(`❌ Could not find llama-helper directory at ${helperDir}`);
  process.exit(1);
}

const hostLine = execSync('rustc -vV', { encoding: 'utf8' })
  .split('\n')
  .find((line) => line.startsWith('host:'));

if (!hostLine) {
  console.error('❌ Failed to detect Rust target triple (rustc -vV)');
  process.exit(1);
}

const targetTriple = hostLine.split(':')[1].trim();
const isWindows = os.platform() === 'win32';
const baseBinary = `llama-helper${isWindows ? '.exe' : ''}`;
const sidecarBinary = `llama-helper-${targetTriple}${isWindows ? '.exe' : ''}`;
const srcPath = path.join(targetDir, baseBinary);
const destPath = path.join(binariesDir, sidecarBinary);

if (!fs.existsSync(binariesDir)) {
  fs.mkdirSync(binariesDir, { recursive: true });
}

if (fs.existsSync(destPath)) {
  console.log(`✅ Sidecar already staged: ${destPath}`);
  process.exit(0);
}

let feature = '';
if (process.env.TAURI_GPU_FEATURE) {
  feature = process.env.TAURI_GPU_FEATURE.trim();
} else if (shouldDetectGpu) {
  try {
    feature = execSync('node scripts/auto-detect-gpu.js', {
      cwd: path.join(repoRoot, 'frontend'),
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'inherit']
    }).trim();
  } catch (err) {
    feature = '';
  }
}

const featureArgs = feature && feature !== 'none' ? ` --features ${feature}` : '';
const profileArg = isRelease ? ' --release' : '';

console.log('🦙 Building llama-helper sidecar...');
console.log(`   Target: ${targetTriple}`);
console.log(`   Profile: ${isRelease ? 'release' : 'debug'}`);
console.log(`   Features: ${feature || '(none)'}`);

try {
  execSync(`cargo build -p llama-helper${profileArg}${featureArgs}`, {
    cwd: repoRoot,
    stdio: 'inherit'
  });
} catch (err) {
  process.exit(err.status || 1);
}

if (!fs.existsSync(srcPath)) {
  console.error(`❌ llama-helper binary not found at ${srcPath}`);
  process.exit(1);
}

// Clean old sidecar copies to avoid confusing bundles
fs.readdirSync(binariesDir)
  .filter((name) => name.startsWith('llama-helper'))
  .forEach((name) => fs.unlinkSync(path.join(binariesDir, name)));

fs.copyFileSync(srcPath, destPath);
console.log(`✅ Copied sidecar to ${destPath}`);
