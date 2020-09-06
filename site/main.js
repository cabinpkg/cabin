require("./scss/style.scss");

const { Elm } = require( './elm/Main.elm' );
const flags = { api: "" };
const app = Elm.Main.init({ flags: flags });


const client = algoliasearch('IOCVK5FECM', '9c0a76bacf692daa9e8eca2aaff4b2ab');
const index = client.initIndex('packages');
app.ports.suggest.subscribe(() => {
    requestAnimationFrame(() => {
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

const search = instantsearch({
    // Replace with your own values
    appId: 'IOCVK5FECM',
    apiKey: '9c0a76bacf692daa9e8eca2aaff4b2ab', // search only API key, no ADMIN key
    indexName: 'packages',
    urlSync: true, // This will keep the browser url in sync and allow users to copy paste urls corresponding to the current search state.
    searchParameters: {
        hitsPerPage: 20,
        distinct: true
    }
});
var isSearchable = false;
app.ports.instantsearch.subscribe(() => {
    requestAnimationFrame(() => {
        search.addWidget(
            instantsearch.widgets.searchBox({
                container: '#search-input'
            })
        );
        search.addWidget(
            instantsearch.widgets.stats({
                container: '#search-top-container',
                templates: {
                    body: "{{nbHits}} packages found"
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.refinementList({
                container: '#cpp-version',
                attributeName: 'package.cpp',
                operator: 'or',
                limit: 10,
                templates: {
                    header: 'C++ version'
                }
            })
        );
        // search.addWidget(
        //     instantsearch.widgets.refinementList({
        //         container: '#package-type',
        //         attributeName: 'package_type',
        //         operator: 'or',
        //         limit: 10,
        //         templates: {
        //             header: 'Package Type'
        //         }
        //     })
        // );
        search.addWidget(
            instantsearch.widgets.hits({
                container: '#hits',
                templates: {
                    item: document.getElementById('hit-template').innerHTML,
                    empty: "We didn't find any results for the search <em>\"{{query}}\"</em>"
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.pagination({
                container: '#pagination'
            })
        );
        search.addWidget(
            instantsearch.widgets.analytics({
                pushFunction: () => {
                    // Google Analytics
                    window.ga('set', 'page', window.location.pathname + window.location.search);
                    window.ga('send', 'pageView');
                }
            })
        );

        if (!isSearchable) {
            // Add this after all the search.addWidget() calls
            search.start();
            // Call start only once
            isSearchable = true;
        }
    });
});
