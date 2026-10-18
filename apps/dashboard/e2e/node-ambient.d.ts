declare var process: {
  env: Record<string, string | undefined>;
};

interface Buffer extends Uint8Array {
  toString(encoding?: string): string;
}

declare var Buffer: {
  concat(list: ReadonlyArray<Uint8Array>): Buffer;
};

declare module 'node:stream' {
  export interface Readable {
    on(event: 'data', listener: (chunk: Buffer) => void): this;
  }
  export interface Writable {
    write(chunk: string): boolean;
  }
  export interface Duplex extends Readable, Writable {}
}

declare module 'node:child_process' {
  import type { Duplex, Readable, Writable } from 'node:stream';
  export interface ChildProcess {
    stdio: [
      Writable | null,
      Readable | null,
      Readable | null,
      Writable | Readable | Duplex | null | undefined,
      Writable | Readable | Duplex | null | undefined,
    ];
    kill(signal?: string): boolean;
    once(event: 'exit', listener: () => void): this;
  }
  export function spawn(
    command: string,
    args: ReadonlyArray<string>,
    options: { stdio: Array<'ignore' | 'pipe'> },
  ): ChildProcess;
}

declare module 'node:fs' {
  export function existsSync(path: string): boolean;
  export function mkdirSync(path: string, options?: { recursive?: boolean; mode?: number }): void;
  export function mkdtempSync(prefix: string): string;
  export function readdirSync(path: string): string[];
  export function readFileSync(path: string): Buffer;
  export function readFileSync(path: string, encoding: 'utf8'): string;
  export function rmSync(path: string, options?: { recursive?: boolean; force?: boolean }): void;
  export function statSync(path: string): { isDirectory(): boolean };
}

declare module 'node:http' {
  export interface IncomingMessage {
    method?: string;
    url?: string;
    headers: Record<string, string | string[] | undefined>;
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
  export function extname(path: string): string;
  export function join(...paths: string[]): string;
  export function normalize(path: string): string;
  export function resolve(...paths: string[]): string;
}

declare module 'node:url' {
  export function fileURLToPath(url: URL | string): string;
}
