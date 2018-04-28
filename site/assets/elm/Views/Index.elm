module Views.Index exposing (indexView)

import Routing exposing (Route(..))
import Views.Common exposing (warningMessage)
import Views.Contact exposing (contactView)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


indexView : Model -> Html Msg
indexView model =
    div [ class "index" ]
        [ headerView, topView, searchView model ]


headerView : Html Msg
headerView =
    header [ class "header" ]
        [ nav []
            [
                li [ class "pull-left" ]
                    [ a [ onClick <| NavigateTo HomeIndexRoute, style [ ("cursor", "pointer") ] ]
                        [ text "poacpm" ]
                    ],
                li []
                    [ a [href "/packages"] [text "Packages"] ],
                li []
                    [ a [href "/donation"] [text "Donation"] ],
                li []
                    [ a [href "/docs"] [text "Documentation"] ],
                li []
                    [ a [href "/signin"] [text "Signin"],
                      text " or ",
                      a [href "/signup"] [text "Signup"] ]
            ]
        ]

topView : Html Msg
topView =
    div []
        [ h1 []
            [ text "Modern Package Manager for C++ Developers" ],
          h2 []
            [ text "poacpm is the package manager that for open source." ],
          h2 []
            [ text "Easy to introduce to your C++ project, you can use the package intuitively." ]
         ]


searchView : Model -> Html Msg
searchView model =
    div [ class "search" ]
        [ input [ placeholder "Find Packages", onInput HandleSearchInput ] [],
          i [ class "fas fa-search search-icon" ] [],
          br [] [],
          a [] [ text (toString model.search) ]
        ]
