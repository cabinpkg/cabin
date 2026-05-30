export function debounce(fn: () => void, ms: number): () => void {
    let timer: ReturnType<typeof setTimeout> | undefined;
    return () => {
        if (timer !== undefined) {
            clearTimeout(timer);
        }
        timer = setTimeout(fn, ms);
    };
}
