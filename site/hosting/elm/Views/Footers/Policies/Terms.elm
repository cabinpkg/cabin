module Views.Footers.Policies.Terms exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)

view : List (Html Msg)
view =
    [ h2 [] [ text "Account" ]
    , div [ class "account-delete-account" ]
        [ h3 [] [ text "Revoke Account" ]
        , hr [ class "head-hr" ] []
        , p []
            [ text "Please revoke access permissions from GitHub." ]
        , p []
            [ text "Sign out from menu after revoke." ]
        , div [ class "margin-top-30" ]
            [ a
                [ class "common-button background-color-red"
                , href "https://github.com/settings/connections/applications/252ce788233ba9b0436d"
                , target "_blank"
                ]
                [ text "Jump to GitHub settings" ]
            ]
        ]
    ]
