// Screenshot a route of the demo UI: node site/demo/shot.mjs /sessions out.png [mobile]
import { launch, UI, sleep } from './lib.mjs';

const [route = '/', out = 'site/demo/out/shot.png', mobile] = process.argv.slice(2);
const { browser, page } = await launch({ mobile: mobile === 'mobile' });
await page.goto(UI + route, { waitUntil: 'networkidle' });
await sleep(1500);
await page.screenshot({ path: out });
await browser.close();
console.log('wrote', out);
