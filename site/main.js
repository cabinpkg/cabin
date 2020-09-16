const { Elm } = require('./src/Main.elm');
const flags = {
    width: window.innerWidth,
    algoliaApplicationId: 'IOCVK5FECM',
    algoliaApiKey: '9c0a76bacf692daa9e8eca2aaff4b2ab',
    algoliaIndexName: 'packages',
};
const app = Elm.Main.init({ flags: flags });
