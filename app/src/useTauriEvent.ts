import { useEffect } from "react";
import { listenEvent } from "./api";

/** Subscribes to a backend event for as long as the component is mounted (no-op outside Tauri). */
export function useTauriEvent<T>(name: string, handler: (payload: T) => void) {
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenEvent<T>(name, handler).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [name, handler]);
}
