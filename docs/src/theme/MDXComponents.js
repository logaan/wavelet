import MDXComponents from '@theme-original/MDXComponents';
import Playground from '@site/src/components/Playground';

// Make <Playground> available in every .md / .mdx file without an explicit
// import.
export default {
  ...MDXComponents,
  Playground,
};
