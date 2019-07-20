module Page.Footer exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Route


view : Html Msg
view =
    footer []
        [ hr [ class "divider-90" ] []
        , div [ class "links" ]
              [ a [ href "mailto:support@poac.pm?subject=[Feedback]" ]
                  [ text "Feedback" ]
              , a [ href "https://github.com/poacpm" ]
                  [ text "GitHub" ]
              , a [ Route.href Route.Policy ]
                  [ text "Policies" ]
--              , a [ href <| Routing.pathFor SponsorsRoute ]
--                  [ text "Sponsors" ]
              ]
        ]
