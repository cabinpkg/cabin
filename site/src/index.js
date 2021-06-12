import { Elm } from './Main.elm';
import * as serviceWorker from './serviceWorker';

const isDarkTheme = window.matchMedia('(prefers-color-scheme: dark)');
const flags = {
    width: window.innerWidth,
    isDarkTheme: isDarkTheme.matches,
    algoliaApplicationId: 'IOCVK5FECM',
    algoliaApiKey: '9c0a76bacf692daa9e8eca2aaff4b2ab',
    algoliaIndexName: 'packages',
};
const app = Elm.Main.init({
    node: document.getElementById('root'),
    flags: flags
});

try {
    // Chrome & Firefox
    isDarkTheme.addEventListener('change', (ev) => {
        app.ports.onThemeChange.send(ev.matches);
    });
} catch (e1) {
    try {
        // Safari
        isDarkTheme.addListener((ev) => {
            app.ports.onThemeChange.send(ev.matches);
        });
    } catch (e2) {
        console.error(e1, e2);
    }
}

// If you want your app to work offline and load faster, you can change
// unregister() to register() below. Note this comes with some pitfalls.
// Learn more about service workers: https://bit.ly/CRA-PWA
serviceWorker.unregister();
