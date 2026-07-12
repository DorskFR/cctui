import type { CodegenConfig } from "@graphql-codegen/cli";

const config: CodegenConfig = {
  schema: [{ "schema/github.graphql": { assumeValidSDL: true, assumeValid: true } }],
  documents: ["src/graphql/**/*.graphql"],
  generates: {
    "src/generated/github-graphql.ts": {
      plugins: ["typescript-operations"],
      config: {
        onlyOperationTypes: true,
        preResolveTypes: true,
        skipTypename: true,
        avoidOptionals: false,
        scalars: { DateTime: "string", URI: "string", HTML: "string" },
      },
    },
  },
};

export default config;
