/* @refresh reload */
import { render } from "solid-js/web";
import { Router, Route } from "@solidjs/router";
import "@maurice/ui/tokens.css";
import "./styles.css";
import { Layout } from "./components/Layout";
import { Home } from "./pages/Home";
import { Workspace } from "./pages/Workspace";
import { ServiceView } from "./pages/Service";
import { Login } from "./pages/Login";
import { Settings } from "./pages/Settings";

render(
  () => (
    <Router root={Layout}>
      <Route path="/login" component={Login} />
      <Route path="/settings" component={Settings} />
      <Route path="/" component={Home} />
      <Route path="/ws/:id" component={Workspace} />
      <Route path="/ws/:wsId/svc/:id" component={ServiceView} />
    </Router>
  ),
  document.getElementById("root")!,
);
