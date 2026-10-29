'use strict'

// 将 npm 实际打包内容安装到独立消费项目，编译并运行同一份 TypeScript 样例。
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')
const root = path.resolve(__dirname, '..')
const sample = fs.mkdtempSync(path.join(os.tmpdir(), 'tdb-npm-consumer-'))
const npmCli = process.env.npm_execpath
assert(npmCli && fs.existsSync(npmCli), '请通过 npm run test:package 运行产物验收')
const tsc = require.resolve('typescript/lib/tsc.js')

function run(args, cwd) {
  const result = spawnSync(process.execPath, args, { cwd, encoding: 'utf8', timeout: 120000 })
  assert.ifError(result.error)
  assert.equal(result.status, 0, `产物验收命令失败：${result.stdout}\n${result.stderr}`)
  return result.stdout
}

try {
  const packed = JSON.parse(run([npmCli, 'pack', '--json', '--ignore-scripts', '--pack-destination', sample], root))
  assert.equal(packed.length, 1, '打包必须产生一个 npm 制品')
  const consumer = path.join(sample, 'consumer')
  fs.mkdirSync(consumer)
  fs.writeFileSync(path.join(consumer, 'package.json'), JSON.stringify({ name: 'tdb-consumer-test', private: true }))
  run([npmCli, 'install', '--ignore-scripts', '--no-audit', '--no-fund', '--package-lock=false',
    path.join(sample, packed[0].filename)], consumer)
  fs.copyFileSync(path.join(root, 'tests/node/types/consumer.ts'), path.join(consumer, 'consumer.ts'))
  const config = JSON.parse(fs.readFileSync(path.join(root, 'tests/node/types/tsconfig.json'), 'utf8'))
  delete config.compilerOptions.paths
  config.compilerOptions.noEmit = false
  config.compilerOptions.outDir = './compiled'
  fs.writeFileSync(path.join(consumer, 'tsconfig.json'), JSON.stringify(config))
  run([tsc, '--project', 'tsconfig.json'], consumer)
  // 向已安装制品注入未定义的公共类型，确认真实编译配置会拒绝损坏声明。
  const installed = path.join(consumer, 'node_modules/triviumdb')
  const metadata = JSON.parse(fs.readFileSync(path.join(installed, 'package.json'), 'utf8'))
  assert.equal(typeof metadata.types, 'string', 'npm 制品必须公开 TypeScript 声明入口')
  const declaration = path.join(installed, metadata.types)
  const original = fs.readFileSync(declaration, 'utf8')
  try {
    fs.writeFileSync(declaration, original + '\nexport declare const typeGateProbe: TdbMissingContractType;\n')
    const rejected = spawnSync(process.execPath, [tsc, '--project', 'tsconfig.json'],
      { cwd: consumer, encoding: 'utf8', timeout: 120000 })
    assert.ifError(rejected.error)
    assert.notEqual(rejected.status, null, '损坏声明的编译验证必须正常完成')
    assert.notEqual(rejected.status, 0, '当前编译配置放过了未定义的公共类型')
  } finally {
    fs.writeFileSync(declaration, original)
  }
  run(['-e', 'process.exitCode = 1; require("./compiled/consumer.js").exercise(process.argv[1]).then(() => { process.exitCode = 0 }, error => { console.error(error) })',
    path.join(sample, 'typed-consumer.tdb')], consumer)
  const numericRoot = path.join(sample, 'numeric-boundaries')
  fs.mkdirSync(numericRoot)
  run(['-e', 'process.exitCode = 1; require(process.argv[1])(process.argv[2], require("triviumdb").TriviumDB).then(() => { process.exitCode = 0 }, error => { console.error(error) })',
    path.join(root, 'tests/node/numeric_boundaries.js'), numericRoot], consumer)
  const kernelRoot = path.join(sample, 'kernel-boundaries')
  fs.mkdirSync(kernelRoot)
  run(['-e', 'process.exitCode = 1; const api = require("triviumdb"); require(process.argv[1]).runKernelBoundaries(process.argv[2], api.TriviumDB, api.ResourceRuntime); process.exitCode = 0',
    path.join(root, 'tests/node/kernel_boundaries.js'), kernelRoot], consumer)
  console.log('npm 产物的 TypeScript 正反样例及原生运行行为验收通过')
} finally {
  fs.rmSync(sample, { recursive: true, force: true })
}
