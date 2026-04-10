type ApiResult<T> = {
  data: T;
  ok: boolean;
  receivedAt: Date;
};

interface User {
  id: string;
  name: string;
  email?: string;
  roles: ("admin" | "editor" | "viewer")[];
}

const defaultUser: Readonly<User> = {
  id: "u_123",
  name: "Ada Lovelace",
  roles: ["viewer"],
};

function formatUser<T extends User>(user: T): string {
  const email = user.email ?? "no-email@example.com";
  return `${user.name} <${email}> [${user.roles.join(", ")}]`;
}

async function fetchUser(id: string): Promise<ApiResult<User>> {
  const response = await fetch(`/api/users/${id}`);
  const data = (await response.json()) as User;

  return {
    data,
    ok: response.ok,
    receivedAt: new Date(),
  };
}

function assertNever(value: never): never {
  throw new Error(`Unexpected value: ${String(value)}`);
}

export { assertNever, defaultUser, fetchUser, formatUser };
