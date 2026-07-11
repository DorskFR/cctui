import { createOctokit, type OctokitRequest } from "./client.ts";
import { type BudgetOptions, BudgetTracker } from "./ratelimit.ts";

export interface Account {
  login: string;
  octokit: OctokitRequest;
  budget: BudgetTracker;
}

export interface AccountInput {
  login: string;
  token: string | undefined;
  octokit?: OctokitRequest;
  budget?: BudgetOptions;
}

export function createAccount(input: AccountInput): Account {
  return {
    login: input.login,
    octokit: input.octokit ?? createOctokit(input.token),
    budget: new BudgetTracker(input.budget ?? { limit: 5000, ceilingFraction: 0.2 }),
  };
}
