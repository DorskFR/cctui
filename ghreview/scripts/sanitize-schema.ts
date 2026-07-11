import { type FieldDefinitionNode, parse, print } from "graphql";

const INVALID_DEPRECATIONS: Record<string, Set<string>> = {
  Project: new Set(["id"]),
  ProjectCard: new Set(["id"]),
  ProjectColumn: new Set(["id"]),
  PullRequest: new Set(["databaseId"]),
  PullRequestReview: new Set(["databaseId"]),
  PullRequestReviewComment: new Set(["databaseId"]),
  Team: new Set(["viewerCanSubscribe", "viewerSubscription"]),
};

const path = new URL("../schema/github.graphql", import.meta.url);
const sdl = await Bun.file(path).text();
const doc = parse(sdl, { noLocation: true });

function stripField(typeName: string, field: FieldDefinitionNode): FieldDefinitionNode {
  const fields = INVALID_DEPRECATIONS[typeName];
  if (!fields?.has(field.name.value)) return field;
  return {
    ...field,
    directives: (field.directives ?? []).filter((d) => d.name.value !== "deprecated"),
  };
}

const next = {
  ...doc,
  definitions: doc.definitions.map((def) => {
    if (def.kind !== "ObjectTypeDefinition" || !INVALID_DEPRECATIONS[def.name.value]) return def;
    return { ...def, fields: (def.fields ?? []).map((f) => stripField(def.name.value, f)) };
  }),
};

await Bun.write(path, `${print(next)}\n`);
console.log("sanitized schema/github.graphql");
