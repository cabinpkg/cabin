module Views.Index exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Json.Decode as Json
import Messages exposing (..)
import Model exposing (..)
import Views.Svgs as Svgs


view : Model -> Html Msg
view model =
    main_ [ class "index" ]
        [ topView model
        , getStartedView model.isFadein
        , section model.isFadein
        ]


topView : Model -> Html Msg
topView model =
    div [ class "top" ]
        [ phraseView
        , Svgs.top model.width
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
        , searchBox
        ]


searchBox : Html Msg
searchBox =
    div [ class "aa-input-container"
        , id "aa-input-container"
        ]
        [ Html.form []
            [ input
                [ type_ "search"
                , id "aa-search-input"
                , class "aa-input-search"
                , placeholder "Search packages"
                , name "search"
                , autocomplete False
                , onKeyDown Search
                , onInput OnSearchInput
                ]
                []
            ]
        ]


onKeyDown : (Int -> msg) -> Attribute msg
onKeyDown tagger =
    on "keydown" (Json.map tagger keyCode)


addFadeinClass : Bool -> String
addFadeinClass bool =
    if bool then
        " fadein scrollin"

    else
        " fadein"


transitionDelay : String -> Attribute msg
transitionDelay time =
    style "transition-delay" time


getStartedView : IsFadein -> Html Msg
getStartedView isFadein =
    div [ class <| "abstract" ++ addFadeinClass isFadein.getStart ]
        [ h3 [] [ text "Useful Information" ]
        , h1 [] [ text "Getting started" ]
        , p []
            [ text """Want to use poac right now?
                    Let's get started in just a sec!
                    If you unless one does use peculiar OS,
                     that you can install poac easily by following command.
                    """
            ]
        , p [ class "code-block" ]
            [ text "$ curl -fsSL https://sh.poac.pm | bash"
            ]
        , p [ class "details" ]
            [ text "Please refer to "
            , a
                [ class "book-link"
                , href "https://docs.poac.io"
                ]
                [ text "The Poac Book" ]
            , text " for details."
            ]
        ]


section : IsFadein -> Html Msg
section isFadein =
    div [ class "section" ]
        [ div [ class <| "card" ++ addFadeinClass isFadein.section1 ]
            [ h2 [] [ text "Useful Interface" ]
            , p []
                [ text """poac is the C++ package manager and
                        poac is the CLI application provided to the client."""
                ]
            , p []
                [ text """poac is easy to use because it refers to the
                        interface of a newer and more used package manager."""
                ]
            ]
        , div
            [ class <| "card" ++ addFadeinClass isFadein.section1
            , transitionDelay ".1s"
            ]
            [ h2 [] [ text "Accelerate Development" ]
            , p []
                [ text """Even if you have used package management manually or
                         using another package manager, you can easily introduce poac."""
                ]
            , p []
                [ text "It also flexibly copes with small-scale development and large-scale."
                ]
            ]
        , div
            [ class <| "card" ++ addFadeinClass isFadein.section1
            , transitionDelay ".2s"
            ]
            [ h2 [] [ text "Open Source Software" ]
            , p []
                [ text "All related to poac is open source."
                ]
            , p []
                [ text """It is possible to make new ones based on this,
                         and it is also possible to contribute to poac."""
                ]
            , p []
                [ text "The client side is written in C++."
                ]
            ]
        ]
