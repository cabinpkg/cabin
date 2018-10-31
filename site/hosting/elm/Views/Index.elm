module Views.Index exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Svg
import Svg.Attributes
import ClarityUI.ProgressBar exposing (..)
import Json.Decode as Json
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    div [ class "index" ]
    [ topView model
    , getStartedView model.isFadein
    , section1View model.isFadein
--    , abstractView model.isFadein
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


--svgView : Svg.Svg Msg
--svgView =
--    Svg.svg [ class "aa-input-icon"
--            , Svg.Attributes.viewBox "654 -372 1664 1664" ]
--            [ Svg.path [ Svg.Attributes.d "M1806,332c0-123.3-43.8-228.8-131.5-316.5C1586.8-72.2,1481.3-116,1358-116s-228.8,43.8-316.5,131.5  C953.8,103.2,910,208.7,910,332s43.8,228.8,131.5,316.5C1129.2,736.2,1234.7,780,1358,780s228.8-43.8,316.5-131.5  C1762.2,560.8,1806,455.3,1806,332z M2318,1164c0,34.7-12.7,64.7-38,90s-55.3,38-90,38c-36,0-66-12.7-90-38l-343-342  c-119.3,82.7-252.3,124-399,124c-95.3,0-186.5-18.5-273.5-55.5s-162-87-225-150s-113-138-150-225S654,427.3,654,332  s18.5-186.5,55.5-273.5s87-162,150-225s138-113,225-150S1262.7-372,1358-372s186.5,18.5,273.5,55.5s162,87,225,150s113,138,150,225  S2062,236.7,2062,332c0,146.7-41.3,279.7-124,399l343,343C2305.7,1098.7,2318,1128.7,2318,1164z" ] []
--            ]


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

--demoView : IsFadein -> Html Msg
--demoView isFadein =
--    div [ class "demo" ] [
--        video [ class <| addFadeinClass isFadein.demo
--              , controls True
--              , poster "/images/thumbnail.png"
--              , height 500
--              ]
--        [ source [ src "/images/demo.mp4"  ] []
--        , source [ src "/images/demo.ogv"  ] []
--        , source [ src "/images/demo.webm" ] []
--        ],
--        div [ class <| "block" ++ addFadeinClass isFadein.demo
--            , transitionDelay ".1s"
--            ] [
--            h3 [] [ text "You should to see" ],
--            h2 [] [ text """Logical colors and
--                            simple expressions""" ],
--            p [] [
--                text """Lorem Ipsum is simply dummy text of
--                         the printing and typesetting industry.
--                        Lorem Ipsum has been the industry's
--                         standard dummy text ever since the 1500s,
--                         when an unkno"""
--            ]
--        ]
--    ]
