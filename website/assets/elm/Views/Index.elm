module Views.Index exposing (indexView, headerView)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


indexView : Model -> Html Msg
indexView model =
    div [ class "index" ] [
        topView model,
        section1View,
        hr [] [],
        section2View,
        section3View,
        footerView
    ]

topView : Model -> Html Msg
topView model =
    div [ class "top" ] [
        headerView model,
        phraseView,
        startView
    ]

headerView : Model -> Html Msg
headerView model =
    header [ class "header" ] [
        nav [] [
            li [ class "pull-left" ] [ aNavLink HomeIndexRoute "poacpm" ],
            li [ class "pull-left" ] [ searchView model ],
            li [] [ aNavLink PackagesRoute "Packages" ],
            li [] [ aNavLink DonateRoute "Donate" ],
            li [] [ a [ href "https://poacpm.github.io/poac/" ] [ text "Docs" ] ],
            li [] [ aLink "Signin", text " or ", aLink "Signup" ]
        ]
    ]

phraseView : Html Msg
phraseView =
    div [ class "text" ] [
        h1 [] [
            text "Modern Package Manager for C++ Developers"
        ],
        h2 [] [
            text "poacpm is the C++ package manager that for open source."
        ],
        h2 [] [
            text "Easy to introduce to your project, you can use the package intuitively."
        ]
    ]

startView : Html Msg
startView =
    div [ class "table" ] [
        a [ href "https://poacpm.github.io/poac/getting-started/installation.html", class "button" ] [
            text "Install poac"
        ],
        a [ href "https://poacpm.github.io/poac/getting-started/", class "button" ] [
            text "Getting Started"
        ]
    ]

searchView : Model -> Html Msg
searchView model =
    div [ class "search" ] [
        input [ placeholder "Search packages", onInput HandleSearchInput ] [],
        br [] [],
        a [] [ text (String.reverse model.search) ]
    ]

section1View : Html Msg
section1View =
    div [ class "section" ] [
        div [ class "table" ] [
            div [ class "image" ] [
                i [ class "fas fa-terminal size-80" ] []
            ],
            div [ class "text" ] [
                h2 [] [ text "Simple and Easy-to-Use Interface" ],
                p [] [
                    text "poacpm is the C++ package manager and poac is the CLI application provided to the client."
                ],
                p [] [
                    text "poac is easy to use because it refers to the interface of a newer and more used package manager."
                ]
            ]
        ]
    ]

section2View : Html Msg
section2View =
    div [ class "section" ] [
        div [ class "table" ] [
            div [ class "text" ] [
                h2 [] [ text "Accelerate development speed" ],
                p [] [
                    text "Even if you have used package management manually or using another package manager, you can easily introduce poac."
                ],
                p [] [
                    text "It also flexibly copes with small-scale development and large-scale development."
                ],
                p [] [
                    text "poac itself is also fast because it is written in C++."
                ]
            ],
            div [ class "image" ] [
                i [ class "fas fa-bolt size-90" ] []
            ]
        ]
    ]

section3View : Html Msg
section3View =
    div [ class "section text-center back-gray" ] [
        div [ class "table" ] [
            div [ class "text" ] [
                h2 [ class "padtop-50" ] [ text "poacpm is Open Source Software" ],
                p [] [
                    text "Even if you have used package management manually or using another package manager, you can easily introduce poac."
                ],
                p [] [
                    text "It also flexibly copes with small-scale development and large-scale development."
                ],
                p [] [
                    text "poac itself is also fast because it is written in C++."
                ]
            ],
            a [ href "https://github.com/poacpm", class "github" ] [
                text "GO TO GITHUB"
            ]
        ]
    ]

footerView : Html Msg
footerView =
    footer [] [
        div [ class "links" ] [
            aLink "Policies",
            aLink "Sponsors",
            aLink "Feedback"
        ],
        div [ class "copyright" ] [
            text "©︎ 2018 Ken Matsui"
        ]
    ]
