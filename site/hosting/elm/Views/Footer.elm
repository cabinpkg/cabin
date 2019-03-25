module Views.Footer exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Routing exposing (..)


view : Html Msg
view =
    footer []
        [ hr [ class "border-hr" ] []
        , div [ class "footer-content" ]
            [ div [ class "links" ]
--              , a [ href ("/" ++ (String.toLower "Sponsors")) ]
--                  [ text "Sponsors"
--                  ]
                [ a [ href "https://patreon.com/matken" ]
                    [ text "Donate" ]
                , a [ href "mailto:support@poac.pm?subject=[Feedback]" ]
                    [ text "Feedback" ]
                , a [ href "https://github.com/poacpm" ]
                    [ text "GitHub" ]
                , a [ href <| Routing.pathFor PolicyRoute ]
                    [ text "Policies" ]
                , a [ href "https://status.poac.pm" ]
                    [ text "Status" ]
                ]
            , div [ class "copyright" ]
                [ text "©︎ 2018 "
                , a [ href "https://github.com/matken11235" ]
                    [ text "Ken Matsui" ]
                ]
            ]
        ]
