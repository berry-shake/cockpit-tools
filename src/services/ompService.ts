import { invoke } from '@tauri-apps/api/core';

export type OmpAccountStatus = 'current' | 'pool' | 'standby' | 'invalid' | 'trash';
export type OmpAccountAction = 'switch' | 'trash' | 'restore';
export interface OmpAccount {
  id: number;
  identity: string;
  provider: string;
  credentialType: string;
  email: string | null;
  accountId: string | null;
  orgName: string | null;
  status: OmpAccountStatus;
  inNativeStore: boolean;
}
export interface OmpState {
  executable: string;
  databasePath: string;
  accounts: OmpAccount[];
  warning: string | null;
  loginSupported: boolean;
}
export const getOmpState = () => invoke<OmpState>('omp_get_state');
export const actOnOmpAccount = (id: number, expectedIdentity: string, action: OmpAccountAction) => invoke<void>('omp_account_action', { id, expectedIdentity, action });
export const loginOmp = (executable: string) => invoke<void>('omp_login', { executable });
