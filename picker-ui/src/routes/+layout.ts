/// SvelteKit prerenders the static HTML at build time. The picker UI
/// is a single-page client app; SSR is irrelevant and a non-prerendered
/// route would require running a Node server which we do not have.
export const prerender = true;
export const ssr = false;
