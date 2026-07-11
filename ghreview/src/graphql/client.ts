import { graphql } from "@octokit/graphql";
import type {
  PullRequestReviewThreadsQuery,
  PullRequestReviewThreadsQueryVariables,
} from "../generated/github-graphql.ts";
import reviewThreadsQuery from "./reviewThreads.graphql" with { type: "text" };

export interface GraphqlClient {
  reviewThreads: (
    vars: PullRequestReviewThreadsQueryVariables,
  ) => Promise<PullRequestReviewThreadsQuery>;
}

export function createGraphqlClient(token: string | undefined): GraphqlClient {
  const client = graphql.defaults({
    headers: token ? { authorization: `token ${token}` } : {},
  });
  return {
    reviewThreads: (vars) =>
      client<PullRequestReviewThreadsQuery>(reviewThreadsQuery, vars as Record<string, unknown>),
  };
}
