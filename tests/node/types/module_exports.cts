import * as commonJs from 'triviumdb';
import {
  TriviumDB, PreparedTql, JsPreparedTql, ResourceRuntime, ConsoleSession,
  RequestContext, verifySnapshot, restoreSnapshot,
} from 'triviumdb';

export const api = {
  TriviumDB, PreparedTql, JsPreparedTql, ResourceRuntime, ConsoleSession,
  RequestContext, verifySnapshot, restoreSnapshot,
};

function check(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

// 两种模块入口使用实际安装包，核对公开值身份及 Prepared 句柄的完整查询结果。
export function exercise(candidate: typeof api, file: string): void {
  for (const name of Object.keys(api) as Array<keyof typeof api>) {
    check(typeof candidate[name] === 'function', `公共值导出不可调用 (Public value export is not callable) ${name}`);
    check(candidate[name] === commonJs[name], `跨模块导出身份不一致 (Cross-module export identity mismatch) ${name}`);
  }
  check(candidate.PreparedTql === candidate.JsPreparedTql, 'Prepared 兼容名称身份不一致 (Prepared alias identity mismatch)');
  const runtime = new candidate.ResourceRuntime({ memoryLimitMb: 64, maxThreads: 1 });
  const db = new candidate.TriviumDB(file, {
    dim: 2, storageMode: 'rom', autoBuildQuiver: false, memoryLimitMb: 16, runtime,
  });
  try {
    db.insertWithId(7, [1, 0], { kind: 'module-exports' });
    const prepared: PreparedTql = db.prepareTql('MATCH (n) RETURN $value AS value');
    const legacy: JsPreparedTql = prepared;
    check(prepared instanceof candidate.PreparedTql, 'Prepared 句柄与规范构造器不匹配 (Prepared handle does not match its public constructor)');
    check(legacy instanceof candidate.JsPreparedTql, 'Prepared 句柄与兼容构造器不匹配 (Prepared handle does not match its compatible constructor)');
    check(JSON.stringify(prepared.parameterNames()) === '["value"]', 'Prepared 参数目录不符 (Prepared parameter catalog mismatch)');
    const context = new candidate.RequestContext('module-exports', 0);
    const rows = context.run(() => db.executePreparedTql(prepared, { value: 42 }));
    check(JSON.stringify(rows) === '[{"value":42}]', 'Prepared 查询内容不符 (Prepared query content mismatch)');
  } finally {
    db.close();
  }
}
