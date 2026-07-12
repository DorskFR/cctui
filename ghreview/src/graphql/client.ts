import { graphql } from "@octokit/graphql";
import type {
  MarkFileAsViewedMutation,
  MarkFileAsViewedMutationVariables,
  PullRequestReviewThreadsQuery,
  PullRequestReviewThreadsQueryVariables,
  PullRequestViewedFilesQuery,
  PullRequestViewedFilesQueryVariables,
  UnmarkFileAsViewedMutation,
  UnmarkFileAsViewedMutationVariables,
} from "../generated/github-graphql.ts";
import markFileViewedMutation from "./markFileViewed.graphql" with { type: "text" };
import pullViewedFilesQuery from "./pullViewedFiles.graphql" with { type: "text" };
import reviewThreadsQuery from "./reviewThreads.graphql" with { type: "text" };
import unmarkFileViewedMutation from "./unmarkFileViewed.graphql" with { type: "text" };

export interface GraphqlClient {
  reviewThreads: (
    vars: PullRequestReviewThreadsQueryVariables,
  ) => Promise<PullRequestReviewThreadsQuery>;
  pullViewedFiles: (
    vars: PullRequestViewedFilesQueryVariables,
  ) => Promise<PullRequestViewedFilesQuery>;
  markFileViewed: (vars: MarkFileAsViewedMutationVariables) => Promise<MarkFileAsViewedMutation>;
  unmarkFileViewed: (
    vars: UnmarkFileAsViewedMutationVariables,
  ) => Promise<UnmarkFileAsViewedMutation>;
}

export function createGraphqlClient(token: string | undefined): GraphqlClient {
  const client = graphql.defaults({
    headers: token ? { authorization: `token ${token}` } : {},
  });
  return {
    reviewThreads: (vars) =>
      client<PullRequestReviewThreadsQuery>(reviewThreadsQuery, vars as Record<string, unknown>),
    pullViewedFiles: (vars) =>
      client<PullRequestViewedFilesQuery>(pullViewedFilesQuery, vars as Record<string, unknown>),
    markFileViewed: (vars) =>
      client<MarkFileAsViewedMutation>(markFileViewedMutation, vars as Record<string, unknown>),
    unmarkFileViewed: (vars) =>
      client<UnmarkFileAsViewedMutation>(unmarkFileViewedMutation, vars as Record<string, unknown>),
  };
}
