#!/usr/bin/env node

'use strict';

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith('--')) continue;
    const key = item.slice(2);
    const value = argv[index + 1] && !argv[index + 1].startsWith('--')
      ? argv[index += 1]
      : 'true';
    args[key] = value;
  }
  return args;
}

function required(value, label) {
  if (!value) throw new Error(`Missing ${label}`);
  return value;
}

function normalizePrefix(prefix) {
  return String(prefix || 'public/art_forge')
    .replace(/^\/+|\/+$/g, '')
    .replace(/\/+/g, '/');
}

function publicUrl(baseUrl, prefix, version, fileName) {
  return `${baseUrl.replace(/\/+$/g, '')}/${prefix}/${version}/${fileName}`;
}

async function artifactMetadata(filePath, label) {
  const resolved = path.resolve(required(filePath, label));
  const stat = fs.statSync(resolved);
  if (!stat.isFile() || stat.size <= 0) {
    throw new Error(`Release artifact is empty or invalid: ${resolved}`);
  }
  const digest = crypto.createHash('sha256');
  for await (const chunk of fs.createReadStream(resolved)) {
    digest.update(chunk);
  }
  return {
    size_bytes: stat.size,
    sha256: digest.digest('hex'),
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const version = required(args.version, '--version').replace(/^v/i, '');
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`Invalid semantic version: ${version}`);
  }

  const output = path.resolve(required(args.output, '--output'));
  const baseUrl = process.env.ALIYUN_OSS_PUBLIC_BASE_URL || 'https://static.honeykid.cn';
  const prefix = normalizePrefix(process.env.ALIYUN_OSS_PREFIX);
  const notes = String(
    process.env.ARTFORGE_RELEASE_NOTES || '本次更新包含功能优化与问题修复。',
  ).trim();
  const files = {
    macos_aarch64: args['macos-aarch64-file'],
    macos_x64: args['macos-x64-file'],
    windows_x64: args['windows-x64-file'],
    windows_x64_portable: args['windows-x64-portable-file'],
  };
  const artifacts = {};
  for (const [platform, filePath] of Object.entries(files)) {
    artifacts[platform] = await artifactMetadata(
      filePath,
      `--${platform.replaceAll('_', '-')}-file`,
    );
  }

  const manifest = {
    version,
    published_at: new Date().toISOString(),
    notes,
    downloads: {
      macos_aarch64: publicUrl(
        baseUrl,
        prefix,
        version,
        'ElunviCanvas_macos_aarch64.dmg',
      ),
      macos_x64: publicUrl(
        baseUrl,
        prefix,
        version,
        'ElunviCanvas_macos_x64.dmg',
      ),
      windows_x64: publicUrl(
        baseUrl,
        prefix,
        version,
        'ElunviCanvas_windows_x64_setup.exe',
      ),
      windows_x64_portable: publicUrl(
        baseUrl,
        prefix,
        version,
        'ElunviCanvas_windows_x64_portable.zip',
      ),
    },
    artifacts,
  };

  fs.mkdirSync(path.dirname(output), { recursive: true });
  fs.writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  console.log(`Update manifest: ${output}`);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
