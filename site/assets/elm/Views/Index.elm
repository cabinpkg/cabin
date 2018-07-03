module Views.Index exposing (view)

import Routing exposing (Route(..))
import Views.Header as Header
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
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
        Header.view model,
        phraseView,
        startView
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
        a [ href "https://poacpm.github.io/poac/en/getting-started/installation.html", class "login pulse" ] [
            text "INSTALL POAC"
        ],
        a [ href "https://poacpm.github.io/poac/en/getting-started/", class "login pulse" ] [
            text "GETTING STARTED"
        ]
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
                    text "All related to poacpm is open source."
                ],
                p [] [
                    text "It is possible to make new ones based on this, and it is also possible to contribute to poacpm."
                ],
                p [] [
                    text "The server side is written in Elixir and the client side is written in C++."
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
            a [ href ("/" ++ (String.toLower "Policies")) ] [
                text "Policies"
            ],
            a [ href ("/" ++ (String.toLower "Sponsors")) ] [
                text "Sponsors"
            ],
            a [ href ("/" ++ (String.toLower "Feedback")) ] [
                text "Feedback"
            ]
        ],
        div [ class "copyright" ] [
            text "©︎ 2018 Ken Matsui"
        ]
    ]
