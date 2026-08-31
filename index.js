'use strict'

// Node 原生模块加载器：只按当前平台、架构和 libc 选择已发布二进制。
// 未命中时返回包含平台信息的明确错误，不静默切换到不兼容 ABI。

const path = require('node:path')

const platform = process.platform
const arch = process.arch
const libc = platform === 'linux' && process.report?.getReport?.().header?.glibcVersionRuntime
  ? 'gnu'
  : 'musl'
const platformArch = platform === 'win32'
  ? `${platform}-${arch}-msvc`
  : platform === 'linux'
    ? `${platform}-${arch}-${libc}`
    : `${platform}-${arch}`
const candidates = [
  `triviumdb.${platformArch}.node`,
  `triviumdb.${platform}-${arch}.node`,
  'triviumdb.node',
]

let binding
let lastError
for (const candidate of candidates) {
  try {
    binding = require(path.join(__dirname, candidate))
    break
  } catch (error) {
    if (error?.code !== 'MODULE_NOT_FOUND') {
      throw error
    }
    lastError = error
  }
}

if (!binding) {
  throw new Error(
    `无法加载 TriviumDB 原生模块 (Unable to load TriviumDB native addon) for ${platformArch}`,
    { cause: lastError },
  )
}

if (binding.TriviumDB) {
  Object.defineProperty(binding.TriviumDB.prototype, 'dispose', {
    configurable: true,
    value() {
      this.close()
    },
  })
  if (typeof Symbol.dispose === 'symbol') {
    Object.defineProperty(binding.TriviumDB.prototype, Symbol.dispose, {
      configurable: true,
      value() {
        this.close()
      },
    })
  }
}

module.exports = binding
