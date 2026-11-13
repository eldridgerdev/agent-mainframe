# Wide Mermaid diagrams

Open this file in the Markdown viewer on a narrow terminal (80 columns is a good
test). Each diagram below is wider than the viewer pane.

## What to try

1. Pan with `h`/`l` or Left/Right. Shift or Ctrl moves 16 columns instead of 4.
2. Look for `◂`/`▸` on clipped rows and `cols a-b of N` in the footer.
3. Press `f` to use the whole terminal, then resize the window while panned.
4. Press `d` to see the source, then `d` again; the pan resets.

This paragraph is wrapped normally and must not move while you pan.

## Flowchart

```mermaid
flowchart LR
    A[Receive request from the client] --> B[Validate the payload schema]
    B --> C[Authenticate the caller token]
    C --> D[Load the account from storage]
    D --> E[Write the audit log entry]
    E --> F[Return the final response]
```

## Sequence

```mermaid
sequenceDiagram
    participant Client
    participant Gateway
    participant Auth
    participant Orders
    participant Billing
    participant Notifier
    Client->>Gateway: Submit the order request
    Gateway->>Auth: Verify the session token
    Auth-->>Gateway: Token accepted
    Gateway->>Orders: Create the order record
    Orders->>Billing: Authorize the payment
    Billing-->>Orders: Payment authorized
    Orders->>Notifier: Send the confirmation email
    Orders-->>Client: Order confirmed
```

## Narrow control

This one fits, so it must render exactly as before with no markers.

```mermaid
flowchart TD
    A[Start] --> B[End]
```
