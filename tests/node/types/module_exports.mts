import {
  TriviumDB, PreparedTql, JsPreparedTql, ResourceRuntime, ConsoleSession,
  RequestContext, verifySnapshot, restoreSnapshot,
} from 'triviumdb';
import { exercise as exerciseApi } from './module_exports.cjs';

export function exercise(file: string): void {
  exerciseApi({
    TriviumDB, PreparedTql, JsPreparedTql, ResourceRuntime, ConsoleSession,
    RequestContext, verifySnapshot, restoreSnapshot,
  }, file);
}
