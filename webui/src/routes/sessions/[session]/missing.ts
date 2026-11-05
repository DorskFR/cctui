export const ARCHIVED_STATUS = 'archived';

export function archivedSearchHref(): string {
	return `/sessions?${new URLSearchParams({ status: ARCHIVED_STATUS })}`;
}
