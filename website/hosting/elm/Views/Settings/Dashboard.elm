module Views.Settings.Dashboard exposing (view)

import Html exposing (..)
import Html.Attributes exposing (..)
import Messages exposing (..)
import Model exposing (..)
--import ClarityUI.ProgressBar as ProgressBar


view : Model -> Html Msg
view model =
    div [ class "content" ]
    [ h2 [] [ text "Dashboard" ]
    , div [ class "dashboard-storage-content" ]
      [ i [ class "fas fa-hdd"
            , style
              [ ("font-size", "20px")
              , ("font-weight", "900")
              , ("margin-left", "20px")
              ]
            ] []
        , span [ class "dashboard-storage" ] [ text "Storage (MB)" ]
        , div [ class "ct-chart" ] []
      ]
    ]
