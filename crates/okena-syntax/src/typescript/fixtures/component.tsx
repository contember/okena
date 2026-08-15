export const Card = <T,>({ value }: { value: T }) => {
  return <section>{format(value)}</section>;
};
