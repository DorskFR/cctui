// The kit owns the text-size levels, the store and the picker. The settings
// blob stores a raw multiplier, so replaying it snaps to the nearest level.
import { fontScale, SCALE_LEVELS } from '@dorsk/tsumikit';

export { fontScale, SCALE_LEVELS };

export function nearestLevel(value: number): string {
	let best = SCALE_LEVELS[0];
	for (const l of SCALE_LEVELS) if (Math.abs(l.value - value) < Math.abs(best.value - value)) best = l;
	return best.id;
}
