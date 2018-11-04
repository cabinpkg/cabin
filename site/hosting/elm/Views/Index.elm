module Views.Index exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Svg
import Svg.Attributes
import ClarityUI.ProgressBar exposing (..)
import Json.Decode as Json
import Messages exposing (..)
import Views.Svgs as Svgs
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "index" ]
    [ topView model
    , getStartedView model.isFadein
    , section1View model.isFadein
--    , abstractView model.isFadein
    ]


topSvg : Html Msg
topSvg =
    Svg.svg
        [ Svg.Attributes.class "top-image"
        , width 800
        , height 600
        , Svg.Attributes.viewBox "0 0 1100 796"
        , Svg.Attributes.version "1.1"
        ]
        [ Svg.defs []
              [ Svg.linearGradient
                    [ Svg.Attributes.x1 "50%"
                    , Svg.Attributes.y1 "0%"
                    , Svg.Attributes.x2 "50%"
                    , Svg.Attributes.y2 "100%"
                    , Svg.Attributes.id "linearGradient-1"
                    ]
                    [ Svg.stop
                          [ Svg.Attributes.stopColor "#FAD961"
                          , Svg.Attributes.offset "0%"
                          ] []
                    , Svg.stop
                          [ Svg.Attributes.stopColor "#F76B1C"
                          , Svg.Attributes.offset "100%"
                          ] []
                    ]
              ]
        , Svg.g
              [ Svg.Attributes.id "top"
              , Svg.Attributes.stroke "none"
              , Svg.Attributes.strokeWidth "1"
              , Svg.Attributes.fill "none"
              , Svg.Attributes.fillRule "evenodd"
              ]
              [ Svg.path
                    [ Svg.Attributes.d
                          """M1048.06933,159.914465 C1035.81036,
                             92.4212078 957.22027,66.5430856 923.002604,
                             63.8577874 C883.887748,60.7881727 842.601285,
                             69.5212209 813.313645,83.0547212 C770.088876,
                             103.028417 740.037464,133.335355 646.172023,
                             136.53134 C548.90982,146.341703 342.277114,
                             56.7953732 276.865748,48.729089 C246.006514,
                             44.923644 231.041145,38.1520814 190.109334,
                             38.1520814 C146.120395,38.1520814 108.899748,
                             48.7039446 87.9310548,78.8051072 C55.7615357,
                             124.985378 72.2843138,180.121257 83.9275815,
                             202.548905 C95.2666361,224.390567 121.74435,
                             249.347569 121.74435,287.527061 C121.74435,
                             321.049304 112.493937,352.461688 82.1646443,
                             389.490524 C40.8168355,439.971795 2.99278216,
                             482.45382 8.842373,516.701646 C26.2369348,
                             618.542259 296.150867,758 548.90982,
                             758 C682.357164,758 840.109536,
                             716.65271 928.069071,671.127651 C1003.84926,
                             631.906233 1083.13166,590.917081 1079.99746,
                             542.00009 C1074.81487,461.112733 912.219418,
                             464.145084 912.219418,388.029995 C912.219418,
                             310.080874 1063.84966,246.795036 1048.06933,
                             159.914465 Z"""
                    , Svg.Attributes.id "Path"
                    , Svg.Attributes.fill "url(#linearGradient-1)"
                    ] []
              , Svg.rect
                    [ Svg.Attributes.id "Rectangle"
                    , Svg.Attributes.fill "#555454"
                    , Svg.Attributes.x "128"
                    , Svg.Attributes.y "147"
                    , Svg.Attributes.width "708"
                    , Svg.Attributes.height "583"
                    , Svg.Attributes.rx "21"
                    ] []
              , Svgs.terminalAnimation
              , Svg.circle
                    [ Svg.Attributes.id "Oval"
                    , Svg.Attributes.fill "#FD5D57"
                    , Svg.Attributes.cx "169"
                    , Svg.Attributes.cy "183"
                    , Svg.Attributes.r "8"
                    ] []
              , Svg.circle
                    [ Svg.Attributes.id "Oval"
                    , Svg.Attributes.fill "#FEBD08"
                    , Svg.Attributes.cx "199"
                    , Svg.Attributes.cy "183"
                    , Svg.Attributes.r "8"
                    ] []
              , Svg.circle
                    [ Svg.Attributes.id "Oval"
                    , Svg.Attributes.fill "#15CD34"
                    , Svg.Attributes.cx "229"
                    , Svg.Attributes.cy "183"
                    , Svg.Attributes.r "8"
                    ] []
              ]
        ]


topView : Model -> Html Msg
topView model =
    div [ class "top" ]
    [ topSvg
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
    div [ class "aa-input-container"
        , id "aa-input-container" ]
        [ Html.form []
            [ input [ type_ "search"
                    , id "aa-search-input"
                    , class "aa-input-search"
                    , placeholder "Search packages"
                    , name "search"
                    , autocomplete False
                    , onKeyDown Search
                    , onInput OnSearchInput ] []
            ]
--        , labeledProgress "10%" 10
--        , svgView
        ]

onKeyDown : (Int -> msg) -> Attribute msg
onKeyDown tagger =
  on "keydown" (Json.map tagger keyCode)


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
            text "Please refer to following link for details:"
        ]
      , a [ href "https://docs.poac.pm" ] [
            text "The Poac Book"
        ]
    ]




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
