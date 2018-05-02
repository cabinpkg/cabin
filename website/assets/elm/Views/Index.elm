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
    div [ class "index" ]
        [ headerView, topView, searchView model ]


headerView : Html Msg
headerView =
    header [ class "header" ]
        [ nav []
            [
                li [ class "pull-left" ] [ aNavLink HomeIndexRoute "poacpm" ],
                li [] [ aLink "Packages" ],
                li [] [ aNavLink DonationRoute "Donation" ],
                li [] [ aLink "Docs" ],
                li [] [ aLink "Signin", text " or ", aLink "Signup" ]
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
          a [] [ text (String.reverse model.search) ]
        ]


descView : Model -> Html Msg
descView model =
    div [] []


footerView : Model -> Html Msg
footerView model =
    footer [] [text "policies", text "sponsors"]
