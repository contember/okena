function listed(value: string): string {
  return value;
}
export { listed as publicListed };

export function* generate<T>(value: T): Generator<T> {
  yield value;
}

export const generated = function* named(value: number): Generator<number> {
  yield value;
};

export enum State {
  Idle,
  Busy = compute(),
}

export namespace Tools {
  export function parse(value: string): string {
    return value;
  }
}

export class SecretBox {
  #secret = makeSecret();
  #hide(): void {}
  visible = makeVisible();
  method(first: string, /* comment is not a parameter */ second = 1): void {}
}
