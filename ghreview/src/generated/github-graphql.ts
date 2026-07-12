/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
/** The possible viewed states of a file . */
export type FileViewedState =
  /** The file has new changes since last viewed. */
  | 'DISMISSED'
  /** The file has not been marked as viewed. */
  | 'UNVIEWED'
  /** The file has been marked as viewed. */
  | 'VIEWED';

export type MarkFileAsViewedMutationVariables = Exact<{
  pullRequestId: string | number;
  path: string;
}>;


export type MarkFileAsViewedMutation = { markFileAsViewed: { pullRequest: { id: string } | null } | null };

export type PullRequestViewedFilesQueryVariables = Exact<{
  owner: string;
  repo: string;
  number: number;
  cursor?: string | null | undefined;
}>;


export type PullRequestViewedFilesQuery = { repository: { pullRequest: { id: string, files: { pageInfo: { hasNextPage: boolean, endCursor: string | null }, nodes: Array<{ path: string, viewerViewedState: FileViewedState } | null> | null } | null } | null } | null };

export type PullRequestReviewThreadsQueryVariables = Exact<{
  owner: string;
  repo: string;
  number: number;
  cursor?: string | null | undefined;
}>;


export type PullRequestReviewThreadsQuery = { repository: { pullRequest: { reviewThreads: { pageInfo: { hasNextPage: boolean, endCursor: string | null }, nodes: Array<{ id: string, isResolved: boolean, isOutdated: boolean, path: string, line: number | null, comments: { nodes: Array<{ id: string, body: string, createdAt: string, author:
                | { login: string }
                | { login: string }
                | { login: string }
                | { login: string }
                | { login: string }
               | null } | null> | null } } | null> | null } } | null } | null };

export type UnmarkFileAsViewedMutationVariables = Exact<{
  pullRequestId: string | number;
  path: string;
}>;


export type UnmarkFileAsViewedMutation = { unmarkFileAsViewed: { pullRequest: { id: string } | null } | null };
