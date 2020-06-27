import "../scss/style.scss";

const db = firebase.firestore();

import { Elm } from "../elm/Main.elm";
const flags = { api: "https://api.poac.pm" };
const app = Elm.Main.init({ flags: flags });


app.ports.fetchOwnPackages.subscribe(async (owner) => {
    const querySnapshot =
        await db.collection("packages")
            .where("owner", "==", owner)
            .get().catch(() => {
                app.ports.receiveOwnPackages.send(null);
            });

    let packages = [];
    let repos = [];
    querySnapshot.forEach((doc) => {
        const data = doc.data();
        if (!repos.includes(data["repo"])) {
            packages.push(JSON.stringify(data));
            repos.push(data["repo"]);
        }
    });
    app.ports.receiveOwnPackages.send(packages);
});

app.ports.fetchPackageVersions.subscribe(async (args) => {
    const [owner, repo] = args;

    const querySnapshot =
        await db.collection("packages")
            .where("owner", "==", owner)
            .where("repo", "==", repo)
            .get().catch(() => {
                app.ports.receiveVersions.send(null);
            });

    let versions = [];
    querySnapshot.forEach((doc) => {
        versions.push(doc.data()["version"]);
    });
    app.ports.receiveVersions.send(versions);
});

app.ports.fetchPackage.subscribe(async (args) => {
    const [owner, repo, version] = args;

    if (version === "latest") {
        const querySnapshot =
            await db.collection("packages")
                .where("owner", "==", owner)
                .where("repo", "==", repo)
                .get().catch(() => {
                    app.ports.receivePackage.send(null);
                });

        let latest = "";
        querySnapshot.forEach((doc) => {
            if (latest < doc.data()["version"]) {
                latest = doc.data()["version"];
            }
        });
        querySnapshot.forEach((doc) => {
            if (latest === doc.data()["version"]) {
                app.ports.receivePackage.send(JSON.stringify(doc.data()));
            }
        });
    }
    else {
        const querySnapshot =
            await db.collection("packages")
                .where("owner", "==", owner)
                .where("repo", "==", repo)
                .where("version", "==", version)
                .get().catch(() => {
                    app.ports.receivePackage.send(null);
                });

        querySnapshot.forEach((doc) => {
            app.ports.receivePackage.send(JSON.stringify(doc.data()));
        });
    }
});


let scroll = window.pageYOffset || document.body.scrollTop;
window.onscroll = () => {
    const newScroll = window.pageYOffset || document.body.scrollTop;
    app.ports.onScroll.send(newScroll);
    scroll = newScroll;
};

app.ports.onWidth.send(window.innerWidth);
window.onresize = () => {
    app.ports.onWidth.send(window.innerWidth);
};


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
                        return '<span>' +
                            suggestion._highlightResult.owner.value + "/" +
                            suggestion._highlightResult.repo.value + '</span>';
                    }
                }
            }
        ).on('autocomplete:selected', function(event, suggestion, dataset) {
            location.href = "/packages/" + suggestion.owner + "/" + suggestion.repo + "/" + suggestion.version;
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
let isSearchable = false;
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
                attributeName: 'cpp_version',
                operator: 'or',
                limit: 10,
                templates: {
                    header: 'C++ version'
                }
            })
        );
        search.addWidget(
            instantsearch.widgets.refinementList({
                container: '#package-type',
                attributeName: 'package_type',
                operator: 'or',
                limit: 10,
                templates: {
                    header: 'Package Type'
                }
            })
        );
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

// Force 3 to convert to 03.
requestAnimationFrame(() => {
    setInterval(() => {
        const q = document.querySelector('[value="3"]');
        if (q) {
            const nq = q.nextSibling;
            if (nq) {
                nq.textContent = " 03 ";
            }
        }
    }, 1000);
});
