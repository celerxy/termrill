import { createContext, useContext } from 'react';

/** Portal 菜单也遵循所属标签的可见状态。 */
export const SessionVisibility = createContext(true);
export const useSessionVisibility = () => useContext(SessionVisibility);
