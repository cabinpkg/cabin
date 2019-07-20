module Page.Footers.Policies exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)

import Route
import Page.Footers.Policies.Conduct as Conduct
import Page.Footers.Policies.Dispute as Dispute
import Page.Footers.Policies.Privacy as Privacy
import Page.Footers.Policies.Terms as Terms
import Page.NotFound as NotFound


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
    div [ class "policies" ] html


mainView : List (Html Msg)
mainView =
    [ h2 [] [ text "Policies" ]
    , div [ class "" ]
          <| List.map2 applyList
          [ "conduct", "dispute", "privacy", "terms" ]
          [ "Code of Conduct", "Dispute Policy", "Privacy Policy", "Terms of Service" ]
    ]

applyList : String -> String -> Html Msg
applyList name display =
    li []
       [ a [ Route.href <| Route.Policies name ]
           [ text display ]
       ]
