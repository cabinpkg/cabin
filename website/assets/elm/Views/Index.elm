module Views.Index exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "index" ]
    [ topView model
    , abstractView model.isFadein
    , section1View model.isFadein
    , demoView model.isFadein
    , getStartedView model.isFadein
    ]

topView : Model -> Html Msg
topView model =
    div [ class "top" ]
    [ img [ class "top-image"
          , src "/images/top.svg"
          , alt "top", width 800, height 600
          ] []
    , phraseView
    , searchBox
    ]

phraseView : Html Msg
phraseView =
    div [ class "text" ]
    [ h1 []
        [ text "Package Manager for C++ Developers"
        ]
    , h2 []
        [ text "poac is the C++ package manager that for open source."
        ]
    , h2 []
        [ text """Easy to introduce to your project,
                   you can use the package intuitively."""
        ]
    ]

searchBox : Html Msg
searchBox =
--    div [] [
    input [ class "search-box"
          , placeholder "Search packages"
          , onInput HandleSearchInput
          ] []
--        a [] [ text (String.reverse model.search) ]
--    ]

addFadeinClass : Bool -> String
addFadeinClass bool =
    if bool then " fadein scrollin" else " fadein"

transitionDelay : String -> Attribute msg
transitionDelay time =
    style [("transition-delay", time)]


abstractView : IsFadein -> Html Msg
abstractView isFadein =
    div [ class <| "abstract" ++ addFadeinClass isFadein.abstract ] [
        h3 [] [ text "User friendly" ],
        h2 [] [
            text "Created this in seek of it."
        ],
        p [] [
            text """C++ is a very historical language.
                    For that reason, it is full of so many assets.
                    However, there is no such deterministic thing
                     with the package manager to handle it easily.
                    Anyway, it is difficult to use existing one.
                    To escape that situation I developed a user
                     friendly package manager."""
        ]
    ]


section1View : IsFadein -> Html Msg
section1View isFadein =
    div [ class "section" ] [
        div [ class <| "card" ++ addFadeinClass isFadein.section1 ] [
            h2 [] [ text "Useful Interface" ],
            p [] [
                text """poac is the C++ package manager and
                        poac is the CLI application provided to the client."""
            ],
            p [] [
                text """poac is easy to use because it refers to the
                        interface of a newer and more used package manager."""
            ]
        ],
        div [ class <| "card" ++ addFadeinClass isFadein.section1
            , transitionDelay ".1s"
            ] [
            h2 [] [ text "Accelerate Development" ],
            p [] [
                text """Even if you have used package management manually or
                         using another package manager, you can easily introduce poac."""
            ],
            p [] [
                text "It also flexibly copes with small-scale development and large-scale."
            ]
        ],
        div [ class <| "card" ++ addFadeinClass isFadein.section1
            , transitionDelay ".2s"
            ] [
            h2 [] [ text "Open Source Software" ],
            p [] [
                text "All related to poac is open source."
            ],
            p [] [
                text """It is possible to make new ones based on this,
                         and it is also possible to contribute to poac."""
            ],
            p [] [
                text "The server side is written in Elixir and the client side is written in C++."
            ]
        ]
    ]

demoView : IsFadein -> Html Msg
demoView isFadein =
    div [ class "demo" ] [
        video [ class <| addFadeinClass isFadein.demo
              , controls True
              , poster "/images/thumbnail.png"
              , height 500
              ]
        [ source [ src "/images/demo.mp4"  ] []
        , source [ src "/images/demo.ogv"  ] []
        , source [ src "/images/demo.webm" ] []
        ],
        div [ class <| "block" ++ addFadeinClass isFadein.demo
            , transitionDelay ".1s"
            ] [
            h3 [] [ text "You should to see" ],
            h2 [] [ text """Logical colors and
                            simple expressions""" ],
            p [] [
                text """Lorem Ipsum is simply dummy text of
                         the printing and typesetting industry.
                        Lorem Ipsum has been the industry's
                         standard dummy text ever since the 1500s,
                         when an unkno"""
            ]
        ]
    ]

getStartedView : IsFadein -> Html Msg
getStartedView isFadein =
    div [ class <| "abstract" ++ addFadeinClass isFadein.getStart ]
      [ h3 [] [ text "Useful Information" ]
      , h2 [] [ text "Getting started" ]
      , p [] [
            text """Want to use poac right now?
                    Let's get started in just a sec!
                    If you unless one does use peculiar OS,
                     that you can install poac easily by following command.
                    """
        ]
      , p [ class "code-block" ] [
            text "$ curl https://sh.poac.pm | bash"
        ]
      , p [] [
            text "More information is following link:"
        ]
      , a [ href "https://docs.poac.pm" ] [
            text "The Poac Book"
        ]
    ]
