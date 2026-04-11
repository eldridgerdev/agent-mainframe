const values = [1, 2, 3, 4];

const summarize = (items) =>
  items.reduce(
    (acc, value) => ({
      total: acc.total + value,
      labels: [...acc.labels, `item-${value}`],
    }),
    { total: 0, labels: [] }
  );

const result = summarize(values);

export default result;
