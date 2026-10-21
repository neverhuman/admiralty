// Minimal Node.js declarations for the TypeScript tooling under scripts/ (and
// packages/contracts/runtime.test.ts). The workspace intentionally has no
// @types/node dependency; declare only the APIs these files use.

declare var process: {
  argv: string[];
  env: Record<string, string | undefined>;
  execPath: string;
  exitCode: number | undefined;
  platform: string;
  version: string;
  exit(code?: number): never;
  getuid?: () => number;
  stdout: { write(chunk: string | Uint8Array): boolean };
  stderr: { write(chunk: string | Uint8Array): boolean };
};

interface Buffer extends Uint8Array {
  toString(encoding?: 'utf8' | 'hex' | 'base64'): string;
}

declare var Buffer: {
  from(input: string, encoding?: 'utf8' | 'base64' | 'hex'): Buffer;
  from(input: ArrayLike<number>): Buffer;
  alloc(size: number): Buffer;
  byteLength(input: string, encoding?: 'utf8'): number;
  concat(list: ReadonlyArray<Uint8Array>): Buffer;
};

declare module 'node:assert/strict' {
  type Expected = RegExp | ((error: unknown) => boolean) | (new (...args: never[]) => unknown) | object;
  interface Assert {
    (value: unknown, message?: string | Error): asserts value;
    ok(value: unknown, message?: string | Error): asserts value;
    equal(actual: unknown, expected: unknown, message?: string | Error): void;
    notEqual(actual: unknown, expected: unknown, message?: string | Error): void;
    deepEqual(actual: unknown, expected: unknown, message?: string | Error): void;
    fail(message?: string | Error): never;
    match(value: string, pattern: RegExp, message?: string | Error): void;
    doesNotMatch(value: string, pattern: RegExp, message?: string | Error): void;
    throws(block: () => unknown, expected?: Expected, message?: string | Error): void;
    rejects(block: Promise<unknown> | (() => Promise<unknown>), expected?: Expected, message?: string | Error): Promise<void>;
  }
  const assert: Assert;
  export default assert;
}

declare module 'node:child_process' {
  type StdioName = 'ignore' | 'pipe' | 'inherit';
  interface SpawnSyncOptions {
    cwd?: string;
    env?: Record<string, string | undefined>;
    input?: string | Uint8Array;
    timeout?: number;
    maxBuffer?: number;
    killSignal?: string;
    stdio?: StdioName | StdioName[];
  }
  export interface SpawnSyncReturns<T> {
    pid: number;
    output: Array<T | null>;
    stdout: T;
    stderr: T;
    status: number | null;
    signal: string | null;
    error?: Error;
  }
  export interface ChildProcess {
    exitCode: number | null;
    signalCode: string | null;
    on(event: 'error', listener: (error: Error) => void): this;
    once(event: 'exit', listener: (...args: unknown[]) => void): this;
    kill(signal?: string): boolean;
  }
  export function spawn(command: string, args: ReadonlyArray<string>, options?: { stdio?: StdioName | StdioName[]; env?: Record<string, string | undefined> }): ChildProcess;
  export function spawnSync(command: string, args: ReadonlyArray<string>, options: SpawnSyncOptions & { encoding: 'utf8' }): SpawnSyncReturns<string>;
  export function spawnSync(command: string, args: ReadonlyArray<string>, options?: SpawnSyncOptions): SpawnSyncReturns<Buffer>;
}

declare module 'node:crypto' {
  interface Hash {
    update(data: string | Uint8Array): Hash;
    digest(encoding: 'hex'): string;
  }
  export function createHash(algorithm: 'sha256'): Hash;
  export function randomUUID(): string;
}

declare module 'node:fs' {
  export interface Stats {
    size: number;
    mode: number;
    uid: number;
    isFile(): boolean;
    isDirectory(): boolean;
  }
  type RemoveOptions = { recursive?: boolean; force?: boolean; maxRetries?: number; retryDelay?: number };
  export const constants: { F_OK: number; R_OK: number; W_OK: number; X_OK: number };
  export function accessSync(path: string, mode?: number): void;
  export function copyFileSync(source: string, destination: string): void;
  export function existsSync(path: string): boolean;
  export function mkdirSync(path: string, options?: { recursive?: boolean; mode?: number }): string | undefined;
  export function mkdtempSync(prefix: string): string;
  export function readdirSync(path: string): string[];
  export function readFileSync(path: string): Buffer;
  export function readFileSync(path: string, encoding: 'utf8'): string;
  export function realpathSync(path: string): string;
  export function rmSync(path: string, options?: RemoveOptions): void;
  export function statSync(path: string): Stats;
  export function writeFileSync(path: string, data: string | Uint8Array, options?: { mode?: number }): void;
}

declare module 'node:fs/promises' {
  import type { Stats } from 'node:fs';
  type RemoveOptions = { recursive?: boolean; force?: boolean; maxRetries?: number; retryDelay?: number };
  export function cp(source: string, destination: string, options?: { recursive?: boolean }): Promise<void>;
  export function lstat(path: string): Promise<Stats>;
  export function mkdir(path: string, options?: { recursive?: boolean; mode?: number }): Promise<string | undefined>;
  export function mkdtemp(prefix: string): Promise<string>;
  export function readdir(path: string): Promise<string[]>;
  export function readFile(path: string | URL): Promise<Buffer>;
  export function readFile(path: string | URL, encoding: 'utf8'): Promise<string>;
  export function rename(source: string, destination: string): Promise<void>;
  export function rm(path: string, options?: RemoveOptions): Promise<void>;
  export function symlink(target: string, path: string, type?: 'dir' | 'file'): Promise<void>;
  export function unlink(path: string): Promise<void>;
  export function writeFile(path: string, data: string | Uint8Array, options?: { flag?: string; mode?: number }): Promise<void>;
}

declare module 'node:http' {
  export interface IncomingMessage { method?: string; url?: string }
  export interface ServerResponse { end(chunk?: string | Uint8Array): this }
  export interface Server {
    listen(port: number, host: string, listening?: () => void): this;
    address(): { port: number; address: string } | string | null;
    close(callback?: (error?: Error) => void): this;
  }
  export function createServer(listener: (request: IncomingMessage, response: ServerResponse) => void): Server;
}

declare module 'node:module' {
  export interface NodeRequire {
    (id: string): unknown;
    resolve(id: string): string;
  }
  export function createRequire(filename: string | URL): NodeRequire;
}

declare module 'node:os' {
  export function tmpdir(): string;
  const os: { tmpdir(): string };
  export default os;
}

declare module 'node:path' {
  interface PlatformPath {
    basename(path: string, suffix?: string): string;
    dirname(path: string): string;
    isAbsolute(path: string): boolean;
    join(...paths: string[]): string;
    relative(from: string, to: string): string;
    resolve(...paths: string[]): string;
    sep: string;
  }
  const path: PlatformPath;
  export default path;
  export const basename: PlatformPath['basename'];
  export const dirname: PlatformPath['dirname'];
  export const join: PlatformPath['join'];
  export const resolve: PlatformPath['resolve'];
}

declare module 'node:test' {
  type TestBody = () => void | Promise<void>;
  function test(name: string, body: TestBody): Promise<void>;
  export default test;
  export { test };
}

declare module 'node:timers/promises' {
  export function setTimeout<T = void>(delay?: number, value?: T): Promise<T>;
}

declare module 'node:url' {
  export function fileURLToPath(url: string | URL): string;
}

declare module 'node:util' {
  interface ParseArgsOption { type: 'string' | 'boolean'; default?: string | boolean; short?: string; multiple?: boolean }
  export function parseArgs(config: {
    args?: string[];
    options: Record<string, ParseArgsOption>;
    strict?: boolean;
    allowPositionals?: boolean;
  }): { values: Record<string, string | boolean | undefined>; positionals: string[] };
}

declare module 'node:zlib' {
  export function inflateSync(data: Uint8Array): Buffer;
  export function deflateSync(data: Uint8Array, options?: { level?: number }): Buffer;
}
