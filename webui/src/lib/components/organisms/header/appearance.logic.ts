export interface ScaleLevelDef {
	id: string;
	label: string;
}

export function scaleLabel(levels: readonly ScaleLevelDef[], id: string): string {
	return levels.find((l) => l.id === id)?.label ?? id;
}
