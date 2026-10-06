import { beforeEach, describe, expect, it, mock } from 'bun:test';

const invokeMock = mock((_: string, __?: Record<string, unknown>) => Promise.resolve(undefined));
const listenMock = mock((_: string, __: (event: { payload: unknown }) => void) =>
	Promise.resolve(() => {}),
);

// Un solo `mock.module` por módulo: dos del mismo dan verde local y rojo en CI.
mock.module('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
mock.module('@tauri-apps/api/event', () => ({ listen: listenMock }));

const state = {
	available: true,
	profiles: ['power-saver', 'balanced', 'performance'],
	activeProfile: 'balanced',
	performanceDegraded: null,
};

describe('power-manager desde el frontend', () => {
	beforeEach(() => {
		invokeMock.mockClear();
		listenMock.mockClear();
	});

	it('pide el estado al comando del plugin', async () => {
		invokeMock.mockResolvedValueOnce(state as never);
		const mod = await import('./index');

		expect(await mod.getPowerState()).toEqual(state);
		expect(invokeMock).toHaveBeenCalledWith('plugin:power-manager|get_power_state');
	});

	it('cambia el perfil con el nombre del argumento que espera Rust', async () => {
		invokeMock.mockResolvedValueOnce({ ...state, activeProfile: 'performance' } as never);
		const mod = await import('./index');

		const next = await mod.setPowerProfile('performance');

		expect(invokeMock).toHaveBeenCalledWith('plugin:power-manager|set_power_profile', {
			profile: 'performance',
		});
		expect(next.activeProfile).toBe('performance');
	});

	it('escucha el evento del plugin y entrega sólo el estado', async () => {
		const mod = await import('./index');
		const seen: unknown[] = [];

		await mod.onPowerStateChanged((s) => seen.push(s));

		expect(listenMock).toHaveBeenCalledTimes(1);
		const [name, callback] = listenMock.mock.calls[0];
		expect(name).toBe('power-profile-changed');
		expect(name).toBe(mod.POWER_STATE_EVENT);

		callback({ payload: state });
		expect(seen).toEqual([state]);
	});
});
