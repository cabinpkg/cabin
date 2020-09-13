const CopyPlugin = require('copy-webpack-plugin');

module.exports = (env, argv) => ({
  entry: {
    index: ['./main.js'],
  },

  output: {
    path: `${__dirname}/dist`,
    filename: 'main.js',
  },

  plugins: [
    new CopyPlugin({
      patterns: [
        { from: 'assets/', to: './' },
        { from: 'index.html', to: './' },
      ],
    }),
  ],

  module: {
    rules: [
      {
        test: /\.elm$/,
        exclude: [/elm-stuff/, /node_modules/],
        loader: "elm-webpack-loader",
      },
    ],
    noParse: [/\.elm$/],
  },
});
