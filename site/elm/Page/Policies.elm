module Page.Policies exposing (view)

import Css exposing (..)
import Html.Styled exposing (..)
import Html.Styled.Attributes exposing (css)
import Messages exposing (..)
import Route
import Page.Policies.Conduct as Conduct
import Page.Policies.Dispute as Dispute
import Page.Policies.Privacy as Privacy
import Page.Policies.Terms as Terms
import Page.NotFound as NotFound



h2Styled : List (Attribute msg) -> List (Html msg) -> Html msg
h2Styled =
    styled h2
        [ marginBottom (10 |> px)
        ]


view : String -> Html Msg
view name =
    case name of
        "" ->
            apply mainView
        "conduct" ->
            apply Conduct.view
        "dispute" ->
            apply Dispute.view
        "privacy" ->
            apply Privacy.view
        "terms" ->
            apply Terms.view
        _ ->
            NotFound.view

apply : List (Html Msg) -> Html Msg
apply html =
    div [ css [ textAlign center ] ] html


mainView : List (Html Msg)
mainView =
    [ h2Styled [] [ text "Policies" ]
    , div []
          <| List.map2 applyList
          [ "conduct", "dispute", "privacy", "terms" ]
          [ "Code of Conduct", "Dispute Policy", "Privacy Policy", "Terms of Service" ]
    ]

applyList : String -> String -> Html Msg
applyList name display =
    li []
       [ a [ Route.href <| Route.Policy name ]
           [ text display ]
       ]
