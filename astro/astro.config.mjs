import { defineConfig } from 'astro/config';
import path from 'node:path';

const docsRoot = path.resolve(process.cwd(), '../docs');
const siteBase = '/spark-stage';

function rewriteDocLinks() {
  return function transform(tree, file) {
    const source = typeof file.path === 'string' ? file.path : '';
    const sourceRelativeDir = path
      .dirname(path.relative(docsRoot, source))
      .replaceAll(path.sep, '/');

    function visit(node) {
      if (node.type === 'link' && typeof node.url === 'string' && node.url.endsWith('.md')) {
        const [target, suffix = ''] = node.url.split(/([#?].*)/, 2);
        const resolved = path.posix.normalize(
          path.posix.join(siteBase, 'docs', sourceRelativeDir, target.replace(/\.md$/, ''))
        );
        const withSlash = resolved.endsWith('/') ? resolved : `${resolved}/`;
        node.url = `${withSlash}${suffix}`;
      }
      if (Array.isArray(node.children)) node.children.forEach(visit);
    }

    visit(tree);
  };
}

export default defineConfig({
  site: 'https://wsafight.github.io',
  base: siteBase,
  output: 'static',
  trailingSlash: 'always',
  markdown: {
    remarkPlugins: [rewriteDocLinks],
  },
});
