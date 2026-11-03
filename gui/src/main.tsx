import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import "./styles.css";
import "./styles/shell.css";
import "./styles/controls.css";
import "./styles/pages.css";
import "./styles/todos.css";
import "./styles/overlays.css";
import "./styles/plan.css";
import "./styles/review.css";
import "./styles/panels.css";
import "./styles/tools.css";
import "./firstRun.css";

const queryClient = new QueryClient();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
