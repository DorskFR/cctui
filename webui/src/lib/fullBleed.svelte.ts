/** Routes that manage their own full-height layout opt out of the width-capped,
 *  padded content column by raising this while they are mounted. */
export const fullBleed = $state({ on: false });

export function holdFullBleed(): () => void {
	fullBleed.on = true;
	return () => {
		fullBleed.on = false;
	};
}
