module Views.Footer exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Routing exposing (..)


view : Html Msg
view =
    footer []
        [ hr [ class "divider-90" ] []
        , div [ class "links" ]
              [ a [ href "mailto:support@poac.pm?subject=[Feedback]" ]
                  [ text "Feedback" ]
              , a [ href "https://github.com/poacpm" ]
                  [ text "GitHub" ]
              , a [ href <| Routing.pathFor PolicyRoute ]
                  [ text "Policies" ]
--              , a [ href <| Routing.pathFor SponsorsRoute ]
--                  [ text "Sponsors" ]
              ]
        ]
