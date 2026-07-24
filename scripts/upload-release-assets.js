#!/usr/bin/env node

'use strict';

const crypto = require('crypto');
const fs = require('fs');
const http = require('http');
const https = require('https');
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

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function stableFileName(fileName, version) {
  const escapedVersion = escapeRegExp(version);
  return fileName
    .replace(new RegExp(`[_-]v?${escapedVersion}(?=[_.-])`, 'g'), '')
    .replace(/__+/g, '_')
    .replace(/--+/g, '-');
}

function publicUrlFor(objectKey) {
  const baseUrl = process.env.ALIYUN_OSS_PUBLIC_BASE_URL || 'https://cdn.honeykid.cn';
  return `${baseUrl.replace(/\/+$/g, '')}/${objectKey}`;
}

function objectUrlFor(objectKey) {
  const bucket = required(process.env.ALIYUN_OSS_BUCKET, 'ALIYUN_OSS_BUCKET');
  const region = required(process.env.ALIYUN_OSS_REGION, 'ALIYUN_OSS_REGION');
  const endpoint = String(process.env.ALIYUN_OSS_ENDPOINT || `${region}.aliyuncs.com`)
    .replace(/^https?:\/\//i, '')
    .replace(/\/+$/g, '');
  const encodedKey = objectKey.split('/').map(encodeURIComponent).join('/');
  return new URL(`https://${bucket}.${endpoint}/${encodedKey}`);
}

function contentTypeFor(filePath) {
  switch (path.extname(filePath).toLowerCase()) {
    case '.dmg':
      return 'application/x-apple-diskimage';
    case '.exe':
      return 'application/vnd.microsoft.portable-executable';
    case '.zip':
      return 'application/zip';
    case '.json':
      return 'application/json; charset=utf-8';
    default:
      return 'application/octet-stream';
  }
}

function signOssRequest({ method, contentType, date, objectKey, ossHeaders }) {
  const bucket = required(process.env.ALIYUN_OSS_BUCKET, 'ALIYUN_OSS_BUCKET');
  const accessKeyId = required(
    process.env.ALIYUN_OSS_ACCESS_KEY_ID,
    'ALIYUN_OSS_ACCESS_KEY_ID',
  );
  const accessKeySecret = required(
    process.env.ALIYUN_OSS_ACCESS_KEY_SECRET,
    'ALIYUN_OSS_ACCESS_KEY_SECRET',
  );
  const canonicalizedOssHeaders = Object.entries(ossHeaders)
    .map(([key, value]) => [key.toLowerCase(), String(value).trim()])
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, value]) => `${key}:${value}\n`)
    .join('');
  const canonicalizedResource = `/${bucket}/${objectKey}`;
  const stringToSign = [
    method,
    '',
    contentType,
    date,
    `${canonicalizedOssHeaders}${canonicalizedResource}`,
  ].join('\n');
  const signature = crypto
    .createHmac('sha1', accessKeySecret)
    .update(stringToSign)
    .digest('base64');
  return `OSS ${accessKeyId}:${signature}`;
}

function putObject(objectKey, filePath) {
  const fileStat = fs.statSync(filePath);
  const method = 'PUT';
  const contentType = contentTypeFor(filePath);
  const date = new Date().toUTCString();
  const ossHeaders = {};
  if (process.env.ALIYUN_OSS_OBJECT_ACL) {
    ossHeaders['x-oss-object-acl'] = process.env.ALIYUN_OSS_OBJECT_ACL;
  }
  const url = objectUrlFor(objectKey);
  const client = url.protocol === 'http:' ? http : https;
  const headers = {
    Date: date,
    'Content-Type': contentType,
    'Content-Length': fileStat.size,
    Authorization: signOssRequest({ method, contentType, date, objectKey, ossHeaders }),
    ...ossHeaders,
  };

  return new Promise((resolve, reject) => {
    const request = client.request(url, { method, headers }, (response) => {
      const chunks = [];
      response.on('data', (chunk) => chunks.push(chunk));
      response.on('end', () => {
        if (response.statusCode >= 200 && response.statusCode < 300) {
          resolve();
          return;
        }
        reject(new Error(
          `OSS upload failed: ${response.statusCode} ${Buffer.concat(chunks).toString('utf8')}`,
        ));
      });
    });
    request.on('error', reject);
    fs.createReadStream(filePath).pipe(request);
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const filePath = path.resolve(required(args.file, '--file'));
  const version = required(args.version, '--version').replace(/^v/i, '');
  if (!fs.existsSync(filePath) || !fs.statSync(filePath).isFile()) {
    throw new Error(`Release artifact does not exist: ${filePath}`);
  }

  required(process.env.ALIYUN_OSS_REGION, 'ALIYUN_OSS_REGION');
  required(process.env.ALIYUN_OSS_BUCKET, 'ALIYUN_OSS_BUCKET');
  required(process.env.ALIYUN_OSS_ACCESS_KEY_ID, 'ALIYUN_OSS_ACCESS_KEY_ID');
  required(process.env.ALIYUN_OSS_ACCESS_KEY_SECRET, 'ALIYUN_OSS_ACCESS_KEY_SECRET');

  const prefix = normalizePrefix(process.env.ALIYUN_OSS_PREFIX);
  const stableName = stableFileName(path.basename(filePath), version);
  const objectKeys = [
    `${prefix}/${version}/${stableName}`,
    `${prefix}/${stableName}`,
  ];

  for (const objectKey of objectKeys) {
    await putObject(objectKey, filePath);
    console.log(`Uploaded ${publicUrlFor(objectKey)}`);
  }
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
