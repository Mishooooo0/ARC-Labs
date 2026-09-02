import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

// Theme is applied before the first paint so the app never flashes the default
// dark ground on the way to a light one.
const saved = localStorage.getItem("arc-labs-theme");
if (saved) document.documentElement.setAttribute("data-theme", saved);

export default mount(App, { target: document.getElementById("app")! });
