export interface PendingArchive {
	title: string;
	message: string;
	ids: string[];
	onDone?: () => void;
}

// One confirm dialog for every bulk archive on the page; the caller supplies
// the wording and the exact ids, and the archive only runs on confirm.
export class ArchiveConfirm {
	pending = $state<PendingArchive | null>(null);
	busy = $state(false);
	#archive: (ids: string[]) => Promise<void>;
	#onerror: (e: unknown) => void;

	constructor(archive: (ids: string[]) => Promise<void>, onerror: (e: unknown) => void) {
		this.#archive = archive;
		this.#onerror = onerror;
	}

	request(p: PendingArchive) {
		if (p.ids.length === 0 || this.busy) return;
		this.pending = p;
	}

	cancel = () => {
		this.pending = null;
	};

	confirm = async () => {
		const p = this.pending;
		if (!p) return;
		this.busy = true;
		try {
			await this.#archive(p.ids);
			p.onDone?.();
		} catch (e) {
			this.#onerror(e);
		} finally {
			this.busy = false;
			this.pending = null;
		}
	};
}
