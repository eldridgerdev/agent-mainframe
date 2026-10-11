# Feature delivery: decisions and retries

```mermaid
flowchart TD
    subgraph Design
        Request([Feature request]) --> Plan[Draft plan]
        Plan --> Approval{Approved?}
        Approval -->|Revise| Plan
    end
    subgraph Implementation
        Code[Implement] --> Tests{Tests pass?}
        Tests -->|No| Fix[Fix issues]
        Fix --> Code
    end
    Approval -->|Yes| Code
    Tests -->|Yes| Review[Final review]
    Review --> Merge([Ready to merge])
```

Two grouped phases, decision diamonds, labelled branches, retry loops and a
shared path to delivery. Scroll vertically to follow the whole flow; press
`d` to inspect the Mermaid source.
