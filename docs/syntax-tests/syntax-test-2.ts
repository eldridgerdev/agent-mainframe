type Message =
  | { kind: "ok"; value: number }
  | { kind: "error"; reason: string };

const toLabel = (message: Message): string => {
  switch (message.kind) {
    case "ok":
      return `value:${message.value.toFixed(2)}`;
    case "error":
      return `error:${message.reason}`;
  }
};

export const currentMessage: Message = { kind: "ok", value: 42.5 };
export const currentLabel = toLabel(currentMessage);
