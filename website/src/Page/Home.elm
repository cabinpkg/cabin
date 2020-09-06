module Page.Home exposing (view)


import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (..)
import Html.Styled.Events exposing (..)
import Json.Decode as Json
import Messages exposing (..)
import Model exposing (..)


view : Model -> Html Msg
view model =
    main_ [ class "home" ]
        [ img [ class "terminal"
              , src "/images/terminal.svg"
              , alt "terminal demonstration"
              ] []
        , phraseView
        , getStartedView model.isFadein
        , section model.isFadein
        ]


phraseView : Html Msg
phraseView =
    div [ class "headline" ]
        [ h1 []
            [ text "Package Manager for C++ Developers"
            ]
        , h2 []
            [ text "Poac is a C++ package manager which is for open source software."
            ]
        , h3 []
            [ text """Easy to introduce to your projects;
                   you can use packages intuitively."""
            ]
        , searchBox
        ]


searchBox : Html Msg
searchBox =
    div [ class "aa-input-container"
        , id "aa-input-container"
        ]
        [ Html.Styled.form []
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
            , label
                [ for "aa-search-input"
                , style "visibility" "hidden"
                , style "display" "block"
                ]
                [ text "Search packages" ]
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
        [ h1 [] [ text "Getting started" ]
        , p []
            [ text """Want to use poac right now?
                    Let's get started in just a sec!
                    Unless, if you use a peculiar an operating system,
                     you can install Poac easily by the following command.
                    """
            ]
        , p [ class "code-block" ]
            [ text "curl -fsSL https://sh.poac.pm | bash"
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
                [ text """Poac is a C++ package manager and
                        a CLI application provided for a client."""
                ]
            , p []
                [ text """Poac is easy to use because it refers to
                        the other package managers' modern interface."""
                ]
            ]
        , div
            [ class <| "card" ++ addFadeinClass isFadein.section1
            , transitionDelay ".1s"
            ]
            [ h2 [] [ text "Accelerate Development" ]
            , p []
                [ text """Even if you have managed packages manually or
                        if you have used other package managers,
                        you can quickly introduce Poac."""
                ]
            , p []
                [ text "Poac flexibly copes with small-scale and large-scale development."
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
                [ text """You can contribute to Poac by publishing packages,
                        or you can contribute to Poac directly."""
                ]
            , p []
                [ text "The client-side is written in C++, and it will be self-hosted."
                ]
            ]
        ]
