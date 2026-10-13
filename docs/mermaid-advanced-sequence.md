# Plan to implementation: four participants

```mermaid
sequenceDiagram
    participant You
    participant AMF
    participant Agent
    participant Git
    You->>AMF: Describe feature
    AMF->>Agent: Draft plan
    Agent-->>AMF: Proposed design
    AMF-->>You: Review plan
    loop Refine plan
        You->>AMF: Feedback
        AMF->>Agent: Revise
        Agent-->>AMF: Updated plan
    end
    You->>AMF: Accept plan
    AMF->>Git: Create worktree
    Git-->>AMF: Isolated branch
    AMF->>Agent: Implement
    alt Checks pass
        Agent-->>AMF: Ready for review
        AMF-->>You: Review changes
    else Checks fail
        Agent-->>AMF: Report failures
        AMF-->>You: Needs attention
    end
```

Four participants, requests and replies, a refinement loop and alternate
success/failure paths. Press `d` to inspect the source. This is an illustrative
workflow; real AMF sessions can take other paths.
