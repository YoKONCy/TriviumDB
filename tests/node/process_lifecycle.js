'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawn, spawnSync } = require('node:child_process')
const { TriviumDB, ResourceRuntime } = require('../../index.js')
const NODES = 300000
const DIM = 32
const next = () => new Promise(resolve => setTimeout(resolve, 5))

function record(directory, phase, data = {}) {
  const event = { phase, time: Date.now(), ...data }
  const target = path.join(directory, phase + '.json')
  fs.writeFileSync(target + '.tmp', JSON.stringify(event))
  fs.renameSync(target + '.tmp', target)
  console.log(JSON.stringify(event))
}

function sample(db, directory, phase) {
  global.gc()
  let linuxMemory = null
  if (process.platform === 'linux') {
    const status = fs.readFileSync('/proc/self/status', 'utf8')
    linuxMemory = Object.fromEntries(['VmHWM', 'RssAnon', 'RssFile', 'RssShmem'].map(field => {
      const matched = status.match(new RegExp(`^${field}:\\s+(\\d+) kB$`, 'm'))
      return [field, matched ? Number(matched[1]) * 1024 : null]
    }))
  }
  record(directory, phase, { memory: process.memoryUsage(), peakRss: process.resourceUsage().maxRSS * 1024,
    linuxMemory, estimated: db.estimatedMemory(), nodes: db.nodeCount() })
}

function seed(db, nodes = NODES, dim = DIM) {
  const vector = Array.from({ length: dim }, (_, axis) => (axis + 1) / dim)
  for (let id = 1; id <= nodes; id++) {
    if (id === nodes) db.setSyncMode('full')
    db.insertWithId(id, vector, { id, revision: 0 })
  }
}

function validate(db, revision = 0) {
  assert.equal(db.nodeCount(), NODES)
  let sum = 0
  for (let id = 1; id <= NODES; id++) {
    const payload = db.getPayload(id)
    assert.equal(payload.id, id)
    assert.equal(payload.revision, id === NODES ? revision : 0)
    sum += payload.id
  }
  assert.equal(sum, NODES * (NODES + 1) / 2)
  for (const id of [1, NODES / 2, NODES]) {
    assert.deepEqual(db.get(id).vector, Array.from({ length: DIM }, (_, axis) => (axis + 1) / DIM))
  }
}

function compileGate(directory) {
  const entered = path.join(directory, '已进入查询')
  const release = path.join(directory, '允许查询结束')
  const source = path.join(directory, 'shutdown_gate.rs')
  const library = path.join(directory, `shutdown_gate.${process.platform === 'win32' ? 'dll' : process.platform === 'darwin' ? 'dylib' : 'so'}`)
  fs.writeFileSync(source, `
    #[no_mangle]
    pub extern "C" fn trivium_hook_abi_version() -> u32 { 2 }
    #[no_mangle]
    pub extern "C" fn trivium_hook_invoke_v2(stage: u32, _: *const f32, _: usize, _: *mut u8, _: usize, _: *mut usize) -> i32 {
      if stage == 1 {
        if std::fs::write(${JSON.stringify(entered)}, b"1").is_err() { return 91; }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !std::path::Path::new(${JSON.stringify(release)}).exists() {
          if std::time::Instant::now() >= deadline { return 92; }
          std::thread::sleep(std::time::Duration::from_millis(1));
        }
      }
      0
    }
  `)
  const compiled = spawnSync('rustc', ['--crate-type=cdylib', source, '-o', library], { encoding: 'utf8' })
  assert.equal(compiled.status, 0, compiled.stderr)
  return { entered, release, library }
}

async function child(mode, directory) {
  assert.equal(typeof global.gc, 'function')
  const file = path.join(directory, 'sample.tdb')
  const runtime = new ResourceRuntime({ maxThreads: 2, maxActiveTasks: 4 })
  const quiver = mode.startsWith('quiver-')
  const dim = quiver ? 2048 : DIM
  const db = new TriviumDB(file, { dim, autoBuildQuiver: quiver, syncMode: 'off', payloadCacheMb: 0, runtime })
  if (mode === 'wal-seed') {
    seed(db)
    sample(db, directory, 'fresh')
    // 跳过析构，独立进程必须仅依靠已确认同步的 WAL 恢复全部节点。
    process.exit(0)
  }
  if (mode === 'wal-replay' || mode === 'checkpoint-open') {
    sample(db, directory, mode)
    validate(db)
    db.close()
    globalThis.closedDatabase = db
    record(directory, mode + '-closed', { status: db.shutdownStatus(), resources: runtime.stats() })
    return
  }
  if (quiver) {
    if (mode === 'quiver-seed') {
      const vector = new Array(dim)
      let state = 0x12345678
      for (let id = 1; id <= 4000; id++) {
        for (let axis = 0; axis < dim; axis++) {
          state ^= state << 13; state ^= state >>> 17; state ^= state << 5
          vector[axis] = (state >>> 0) / 4294967296 - 0.5
        }
        db.insertWithId(id, vector, { id })
      }
      const hits = db.search(vector, 5, 0, -1)
      fs.writeFileSync(path.join(directory, 'quiver-query.json'), JSON.stringify({ vector, hits }))
      db.flush()
    }
    assert.equal(db.nodeCount(), 4000)
    assert(fs.existsSync(file + '.idx'), '已发布文件组必须包含统一索引容器')
    sample(db, directory, mode)
    assert(db.estimatedMemory() < 4000 * dim * 4, '重开应保持紧凑索引与映射向量结构')
    const expected = JSON.parse(fs.readFileSync(path.join(directory, 'quiver-query.json'), 'utf8'))
    assert.deepEqual(db.search(expected.vector, 5, 0, -1), expected.hits, '冷页回收与重开应保持查询结果和分数')
    db.close()
    globalThis.closedDatabase = db
    return
  }

  const busy = mode.endsWith('busy')
  let signalled = false
  process.on('SIGTERM', () => { signalled = true })
  seed(db)
  let pending
  if (busy) {
    db.flush()
    db.updatePayload(NODES, { id: NODES, revision: 1 })
    db.loadFfiHook(path.join(directory, `shutdown_gate.${process.platform === 'win32' ? 'dll' : process.platform === 'darwin' ? 'dylib' : 'so'}`))
    db.enableAutoCompaction(1)
    pending = db.searchBatch([Array.from({ length: DIM }, (_, axis) => (axis + 1) / DIM)], 1, 1, 0)
    pending.catch(() => {})
    while (!fs.existsSync(path.join(directory, '已进入查询'))) await next()
    // 查询屏障阻止写锁，等待周期维护至少发生一次延期后再发出就绪事件。
    const deadline = Date.now() + 30000
    while ((db.instanceStats().compaction?.deferred_runs ?? 0) === 0) {
      assert(Date.now() < deadline, '周期维护未到达查询屏障')
      await next()
    }
  }
  record(directory, 'ready', { nodes: db.nodeCount(), status: db.shutdownStatus() })
  while (!signalled && !(mode === 'drain-busy' && fs.existsSync(path.join(directory, '请求关闭')))) await next()
  record(directory, 'shutdown-triggered', { trigger: mode.startsWith('signal-') ? 'SIGTERM' : 'host-request' })
  db.requestShutdown()
  record(directory, 'draining', { status: db.shutdownStatus() })
  const deadline = Date.now() + 60000
  let previousPhase, nextDiagnostic = 0
  while (!db.closeTimeout(0)) {
    const status = db.shutdownStatus()
    if (status.phase !== previousPhase || Date.now() >= nextDiagnostic) {
      record(directory, 'shutdown-progress', { status, resources: runtime.stats() })
      previousPhase = status.phase
      nextDiagnostic = Date.now() + 5000
    }
    assert(Date.now() < deadline, `关闭排空超出死锁保护期限：${JSON.stringify(status)}`)
    await next()
  }
  if (pending) await pending
  globalThis.closedDatabase = db
  assert.throws(() => db.getPayload(1), /Database closed|数据库已关闭/)
  const resources = runtime.stats()
  assert.equal(resources.attached_databases, 0)
  assert.equal(resources.active_tasks, 0)
  assert.equal(resources.active_maintenance_tasks, 0)
  assert.equal(resources.maintenance_jobs, 0)
  record(directory, 'closed', { status: db.shutdownStatus(), resources })
}

async function main() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'tdb-进程生命周期-'))
  const reports = path.resolve(process.env.TDB_LIFECYCLE_REPORT_DIR || 'target/lifecycle-reports')
  fs.mkdirSync(reports, { recursive: true })
  const results = []
  async function launch(mode, directory, control) {
    fs.mkdirSync(directory, { recursive: true })
    const started = Date.now()
    const proc = spawn(process.execPath, ['--expose-gc', __filename, mode, directory], { stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = '', stderr = '', ended = false
    proc.stdout.on('data', value => { stdout += value })
    proc.stderr.on('data', value => { stderr += value })
    const done = new Promise(resolve => {
      proc.once('error', error => { stderr += error.stack })
      proc.once('close', (code, signal) => { ended = true; resolve({ code, signal }) })
    })
    const watchdog = setTimeout(() => proc.kill('SIGKILL'), 120000)
    try {
      if (control) await control(proc, () => ended)
      const exit = await done
      results.push({ mode, elapsed_ms: Date.now() - started, ...exit, stdout, stderr })
      assert.equal(exit.code, 0, `${mode} 子进程失败：${stderr}\n${stdout}`)
      return stdout.split('\n').filter(line => line.startsWith('{')).map(line => JSON.parse(line))
    } catch (error) {
      if (!ended) proc.kill('SIGKILL')
      const exit = await done
      if (!results.some(result => result.mode === mode)) results.push({ mode, ...exit, stdout, stderr, failure: error.stack })
      throw error
    } finally {
      clearTimeout(watchdog)
      fs.writeFileSync(path.join(reports, 'process-lifecycle.json'), JSON.stringify({ platform: process.platform, root, results }, null, 2))
    }
  }
  try {
    const wal = path.join(root, 'wal')
    await launch('wal-seed', wal)
    const fresh = JSON.parse(fs.readFileSync(path.join(wal, 'fresh.json'), 'utf8'))
    await launch('wal-replay', wal)
    const replay = JSON.parse(fs.readFileSync(path.join(wal, 'wal-replay.json'), 'utf8'))
    await launch('checkpoint-open', wal)
    const checkpoint = JSON.parse(fs.readFileSync(path.join(wal, 'checkpoint-open.json'), 'utf8'))
    assert.equal(replay.estimated, fresh.estimated, 'WAL 恢复后的逻辑堆结构应与原会话相同')
    assert(checkpoint.estimated < NODES * DIM * 4, '检查点重开应保持原始向量映射')
    const indexed = path.join(root, 'quiver')
    await launch('quiver-seed', indexed)
    await launch('quiver-open', indexed)

    const cases = [{ mode: 'drain-busy', busy: true, signal: false }]
    if (process.platform === 'linux') cases.push({ mode: 'signal-idle', busy: false, signal: true }, { mode: 'signal-busy', busy: true, signal: true })
    for (const { mode, busy, signal } of cases) {
        const directory = path.join(root, mode)
        fs.mkdirSync(directory)
        const gate = busy ? compileGate(directory) : undefined
        await launch(mode, directory, async (proc, ended) => {
          async function wait(phase) {
            const deadline = Date.now() + 60000
            const event = path.join(directory, phase + '.json')
            while (!fs.existsSync(event)) {
              assert(!ended(), `子进程在 ${phase} 前退出`)
              assert(Date.now() < deadline, `未等到 ${phase}`)
              await next()
            }
            return JSON.parse(fs.readFileSync(event, 'utf8'))
          }
          await wait('ready')
          if (signal) assert(proc.kill('SIGTERM'), '向本次测试子进程发送 SIGTERM 失败')
          else fs.writeFileSync(path.join(directory, '请求关闭'), '1')
          const draining = await wait('draining')
          if (busy) {
            assert.equal(draining.status.phase, 'draining')
            assert(draining.status.active_operations > 0, '排空期间应保留在途查询')
            fs.writeFileSync(gate.release, '1')
          }
        })
        const reader = new TriviumDB(path.join(directory, 'sample.tdb'), { dim: DIM, autoBuildQuiver: false, payloadCacheMb: 0 })
        validate(reader, busy ? 1 : 0)
        reader.close()
        const moved = directory + '.moved'
        fs.renameSync(directory, moved)
        fs.renameSync(moved, directory)
    }
    if (process.platform !== 'linux') console.log('Linux SIGTERM 用例由 Linux 原生 CI 执行')
    fs.rmSync(root, { recursive: true, force: true })
    console.log('重启内存、全量恢复校验与进程关闭回归通过')
  } catch (error) {
    console.error('进程生命周期回归失败，样本目录保留：', root, error)
    throw error
  }
}

if (process.argv[2]) child(process.argv[2], process.argv[3]).catch(error => { console.error('子进程诊断失败', error); process.exitCode = 1 })
else main().catch(() => { process.exitCode = 1 })
