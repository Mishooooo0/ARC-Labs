/**
 * The force layout, off the main thread.
 *
 * A 5,000-node simulation is tens of milliseconds per tick. On the main thread
 * that is every frame budget in the product spent on physics — typing would
 * stutter, the tree would lag, and Phase 1's latency gate would fail whenever
 * the graph happened to be open. Here it costs the UI thread nothing but a
 * `postMessage` of positions.
 *
 * Positions are shipped as a single `Float32Array` and **transferred**, not
 * copied. At 5,000 nodes a structured clone per tick would allocate 40 KB sixty
 * times a second; transferring hands over the same buffer with no copy at all.
 */

import {
  forceCenter,
  forceLink,
  forceManyBody,
  forceSimulation,
  forceX,
  forceY,
  type Simulation,
  type SimulationLinkDatum,
  type SimulationNodeDatum,
} from "d3-force";

interface Node extends SimulationNodeDatum {
  id: number;
  degree: number;
}
type Link = SimulationLinkDatum<Node>;

let sim: Simulation<Node, Link> | null = null;
let nodes: Node[] = [];

export interface StartMessage {
  type: "start";
  nodes: { id: number; degree: number }[];
  edges: { source: number; target: number }[];
  width: number;
  height: number;
}
export interface StopMessage {
  type: "stop";
}
export interface ReheatMessage {
  type: "reheat";
}
type Incoming = StartMessage | StopMessage | ReheatMessage;

self.onmessage = (event: MessageEvent<Incoming>) => {
  const msg = event.data;

  if (msg.type === "stop") {
    sim?.stop();
    sim = null;
    return;
  }

  if (msg.type === "reheat") {
    sim?.alpha(0.4).restart();
    return;
  }

  const { width, height } = msg;
  nodes = msg.nodes.map((n) => ({ ...n }));
  const links: Link[] = msg.edges.map((e) => ({ source: e.source, target: e.target }));

  sim?.stop();
  sim = forceSimulation<Node, Link>(nodes)
    .force(
      "link",
      forceLink<Node, Link>(links)
        .id((d) => d.id)
        // Denser nodes get slightly longer links, which stops hubs collapsing
        // into an unreadable knot.
        .distance((l) => 20 + Math.min(40, ((l.source as Node).degree ?? 0) * 0.6))
        .strength(0.35),
    )
    // Repulsion is the expensive force. `theta` trades exactness for speed via
    // Barnes-Hut approximation, and `distanceMax` stops every node computing a
    // negligible force against every distant node — together they are most of
    // the difference between 5,000 nodes being interactive and not.
    //
    // Strength scales with the square root of the node count. A fixed value
    // that spaces 200 notes pleasantly collapses 5,000 into an unreadable
    // blob, because link attraction grows with the graph while repulsion does
    // not. Scaling keeps the two in proportion.
    .force(
      "charge",
      forceManyBody<Node>()
        .strength(-18 - Math.sqrt(nodes.length) * 1.6)
        .theta(0.9)
        .distanceMax(600),
    )
    .force("center", forceCenter(width / 2, height / 2))
    // Gentle pull to the middle so disconnected notes — and a vault has many —
    // drift into view instead of being flung to infinity by charge alone.
    .force("x", forceX(width / 2).strength(0.02))
    .force("y", forceY(height / 2).strength(0.02))
    .alphaDecay(0.02);

  const positions = new Float32Array(nodes.length * 2);

  sim.on("tick", () => {
    for (let i = 0; i < nodes.length; i++) {
      positions[i * 2] = nodes[i]!.x ?? 0;
      positions[i * 2 + 1] = nodes[i]!.y ?? 0;
    }
    // A copy per message because the buffer is transferred away each time.
    const out = positions.slice();
    (self as unknown as Worker).postMessage({ type: "tick", positions: out, alpha: sim!.alpha() }, [
      out.buffer,
    ]);
  });

  sim.on("end", () => {
    (self as unknown as Worker).postMessage({ type: "settled" });
  });
};
