const { Elm } = require( './src/Main.elm' );
const flags = {
    width: window.innerWidth,
    algoliaApplicationId: 'IOCVK5FECM',
    algoliaApiKey: '9c0a76bacf692daa9e8eca2aaff4b2ab',
    algoliaIndexName: 'packages',
};
const app = Elm.Main.init({ flags: flags });

const client = algoliasearch('IOCVK5FECM', '9c0a76bacf692daa9e8eca2aaff4b2ab');
const index = client.initIndex('packages');
app.ports.suggest.subscribe(function () {
    requestAnimationFrame(function () {
        //initialize autocomplete on search input (ID selector must match)
        autocomplete('#aa-search-input',
            { hint: true, debug: true }, {
                source: autocomplete.sources.hits(index, {hitsPerPage: 5, distinct: true}),
                //hash of templates used when rendering dataset
                templates: {
                    //'suggestion' templating function used to render a single suggestion
                    suggestion: function(suggestion) {
                        return '<span>' + suggestion._highlightResult.package.name.value + '</span>';
                    }
                }
            }
        ).on('autocomplete:selected', function(event, suggestion, dataset) {
            // TODO: location.href = "/packages/?q=" + suggestion.package.name;
            window.open(suggestion.package.repository, '_blank');
        });
    });
});
