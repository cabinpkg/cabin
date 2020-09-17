var isDarkTheme = window.matchMedia('(prefers-color-scheme: dark)');

var { Elm } = require('./src/Main.elm');
var flags = {
    width: window.innerWidth,
    isDarkTheme: isDarkTheme.matches,
    algoliaApplicationId: 'IOCVK5FECM',
    algoliaApiKey: '9c0a76bacf692daa9e8eca2aaff4b2ab',
    algoliaIndexName: 'packages',
};
var app = Elm.Main.init({ flags: flags });

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
