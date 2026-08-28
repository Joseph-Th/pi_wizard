import { render } from "solid-js/web";

import { App } from "./App";
import "./styles.css";

const root = document.getElementById("root");

if (root === null) {
  throw new Error("Pi Wizard root element is missing");
}

render(() => <App />, root);
