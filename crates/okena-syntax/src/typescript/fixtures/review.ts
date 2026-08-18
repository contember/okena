export async function run<T>(value: T, service: Service): Promise<T> {
  for (const item of [value]) {
    if (service.ready(item)) {
      service.call(value);
    }
  }
  return value;
}

export class Worker<T> {
  value: T;
  execute(value: T): T {
    return value;
  }
}

interface Executor<T> {
  execute(value: T): T;
}

export type Result<T> = { value: T };
