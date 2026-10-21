declare var process: {
  env: Record<string, string | undefined>;
  execPath: string;
  platform: string;
};

interface Buffer extends Uint8Array {
  toString(encoding?: string): string;
}

declare var Buffer: {
  concat(list: ReadonlyArray<Uint8Array>): Buffer;
  byteLength(string: string, encoding?: 'utf8'): number;
  from(input: string, encoding?: 'utf8' | 'base64'): Buffer;
};

declare module 'node:crypto' {
  export interface Hash {
    update(data: Buffer | string): this;
    digest(encoding: 'hex'): string;
  }
  export function createHash(algorithm: 'sha256'): Hash;
}

declare module 'node:module' {
  export interface NodeRequire {
    resolve(id: string): string;
  }
  export function createRequire(filename: string | URL): NodeRequire;
}

declare module 'node:stream' {
  export interface Readable {
    on(event: 'data', listener: (chunk: Buffer) => void): this;
    once(event: 'close', listener: () => void): this;
    once(event: 'error', listener: (err: Error) => void): this;
  }
  export interface Writable {
    write(chunk: string): boolean;
    once(event: 'error', listener: (err: Error) => void): this;
  }
  export interface Duplex extends Readable, Writable {}
}

declare module 'node:child_process' {
  import type { Duplex, Readable, Writable } from 'node:stream';
  export interface ChildProcess {
    stderr?: Readable | null;
    stdio: [
      Writable | null,
      Readable | null,
      Readable | null,
      Writable | Readable | Duplex | null | undefined,
      Writable | Readable | Duplex | null | undefined,
    ];
    kill(signal?: string): boolean;
    once(event: 'error', listener: (err: Error) => void): this;
    once(event: 'exit', listener: (code?: number | null, signal?: string | null) => void): this;
  }
  export function spawn(
    command: string,
    args: ReadonlyArray<string>,
    options: { stdio: Array<'ignore' | 'pipe'>; env?: Record<string, string | undefined> },
  ): ChildProcess;
  export function spawnSync(
    command: string,
    args: ReadonlyArray<string>,
    options?: {
      cwd?: string;
      env?: Record<string, string | undefined>;
      encoding?: 'utf8';
      timeout?: number;
    },
  ): {
    status: number | null;
    stdout: string;
    stderr: string;
  };
}

declare module 'node:fs' {
  export function writeSync(fd: number, data: string): number;
  export function realpathSync(path: string): string;
  export function chmodSync(path: string, mode: number): void;
  export function existsSync(path: string): boolean;
  export function mkdirSync(path: string, options?: { recursive?: boolean; mode?: number }): void;
  export function mkdtempSync(prefix: string): string;
  export function readdirSync(path: string): string[];
  export function readFileSync(path: string): Buffer;
  export function readFileSync(path: string, encoding: 'utf8'): string;
  export function rmSync(
    path: string,
    options?: { recursive?: boolean; force?: boolean; maxRetries?: number; retryDelay?: number },
  ): void;
  export function statSync(path: string): { isDirectory(): boolean; uid: number; mode: number };
  export function writeFileSync(
    path: string,
    data: string | Buffer,
    options?: { encoding?: 'utf8'; mode?: number },
  ): void;
}

declare module 'node:http' {
  export interface IncomingMessage {
    method?: string;
    url?: string;
    headers: Record<string, string | string[] | undefined>;
    rawHeaders?: string[];
    destroy(error?: Error): this;
    on(event: 'data', listener: (chunk: Buffer) => void): this;
    on(event: 'end' | 'close', listener: () => void): this;
  }
  export interface ServerResponse {
    writeHead(statusCode: number, headers?: Record<string, string>): this;
    write(chunk: string | Buffer): boolean;
    end(chunk?: string | Buffer): this;
    destroy(): void;
    on(event: 'close', listener: () => void): this;
  }
  export interface Server {
    once(event: 'error', listener: (err: Error) => void): this;
    listen(port: number, host: string, listeningListener?: () => void): this;
    address(): { port: number; address: string } | string | null;
    close(callback?: (err?: Error) => void): this;
  }
  export function createServer(
    requestListener: (req: IncomingMessage, res: ServerResponse) => void,
  ): Server;
}

declare module 'node:os' {
  export function homedir(): string;
  export function tmpdir(): string;
}

declare module 'node:path' {
  export function dirname(path: string): string;
  export function extname(path: string): string;
  export function join(...paths: string[]): string;
  export function normalize(path: string): string;
  export function resolve(...paths: string[]): string;
}

declare module 'node:url' {
  export function fileURLToPath(url: URL | string): string;
}
